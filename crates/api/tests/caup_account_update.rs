//! CAUP · `COACTUPC` · map `COACTUP`: `PUT /accounts/{id}` through the HTTP
//! layer, row by row of the `COACTUPC` table of
//! `docs/parity/validation-audit.md`.
//!
//! Each test takes its statuses, BMS fields, kinds, orders and failing inputs
//! from the audit rows and its texts from `carddemo_domain::messages`, and
//! checks that the catalog text equals the row's quoted text. Every request
//! starts from the `GET /accounts/00000000001` answer turned back into the
//! `COACTUP` input fields (P-COACTUPC-004, P-COACTUPC-019).
//!
//! # What one request can reach
//!
//! E-26 holds at `halt` every `CSUTLDTC` call `CSUTLDPY` `EDIT-DATE-LE` makes
//! while no message is set, so `account_update::validate` answers such a
//! request with the unexpected-error outcome and writes nothing. `Account
//! Status` is the first edit and `Open Date` the second (P-COACTUPC-006), so:
//!
//! - the rows ranked before the first `CSUTLDTC` row (V-COACTUPC-023) are the
//!   answer's `message`;
//! - every later edit row is sent behind a rejected `Account Status`
//!   (V-COACTUPC-012), whose text is then the `message` and pins the
//!   `CSUTLDTC` outcome (P-CSUTLDPY-014), and the row shows as its own
//!   `fields` entries, as `1200-EDIT-MAP-INPUTS` flags them;
//! - a body that passes every edit is answered 500 with
//!   `messages::UNEXPECTED_ERROR` before any lock or rewrite. The `CSUTLDTC`
//!   rejection rows V-COACTUPC-023, V-COACTUPC-036, V-COACTUPC-049 and
//!   V-COACTUPC-076, the lock, change and rewrite rows V-COACTUPC-124,
//!   V-COACTUPC-125, V-COACTUPC-126, V-COACTUPC-127 and V-COACTUPC-128, and
//!   the write turn P-COACTUPC-008, P-COACTUPC-009, P-COACTUPC-010,
//!   P-COACTUPC-011, P-COACTUPC-012, P-COACTUPC-013, P-COACTUPC-014 and
//!   P-COACTUPC-015 are therefore not reachable over HTTP while E-26 holds;
//!   the rule tests of `carddemo_domain::rules::account_update` prove them
//!   on the validated records. The halt tests below pin the HTTP answer, the
//!   absence of any write, and the E-26 status that answer depends on.
//!
//! # Rows without an HTTP counterpart
//!
//! The 3270 dialogue mechanics P-COACTUPC-001, P-COACTUPC-002,
//! P-COACTUPC-016, P-COACTUPC-017, P-COACTUPC-034, P-COACTUPC-035,
//! P-COACTUPC-036, P-COACTUPC-037, P-COACTUPC-038, P-COACTUPC-039,
//! P-COACTUPC-040, P-COACTUPC-041, P-COACTUPC-042 and P-COACTUPC-046 (keys,
//! COMMAREA, screen variables and attributes, abend) become the one request
//! (E-56); what they decide reaches the answer as the `fields` order and the
//! `message` checked here.
//!
//! # The `CSUTLDPY` and `CSLKPCDY` rows
//!
//! The copybook procedures `COACTUPC` copies have rows of their own in
//! `docs/parity/parity-oracle-audit.md`. The last tests read them by ID: each
//! `CSUTLDPY` edit row is sent for all four dates, through `PUT` and through
//! `carddemo_domain::rules::date_edit`, and each `CSLKPCDY` area-code table is
//! compared with its port and sent as the area codes of both phones.
//!
//! Docker must be running; the harness conventions are those of `common`.
//! Each test starts its own seeded database, so the destructive ones (table
//! renames, deleted rows, the trigger) cannot reach another test.

pub mod common;

use std::collections::BTreeSet;

use carddemo_api::handlers::accounts::MISSING_VERSION_MESSAGE;
use carddemo_audit::{
    Document, Repo, cell_code_spans, citations, code_span, is_no_message, key_values,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_domain::rules::ReturnMsg;
use carddemo_domain::rules::account_update::{VALID_EASY_RECOG_AREA_CODE, VALID_PHONE_AREA_CODE};
use carddemo_domain::rules::date_edit::{
    DateBmsFields, DateEditFlags, DateParts, FlagState, edit_date_ccyymmdd,
};
use carddemo_domain::rules::date_validation::{DatePicture, DateValidationError};
use carddemo_testkit::{block_on, fixtures};
use common::{
    Resp, TestApp, assert_error, fixture_records, parity_audit, validation_audit, validation_ids,
};
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde_json::{Map, Value};

/// The program whose validation rows this file exercises.
const PROGRAM: &str = "COACTUPC";

/// The processing clock: the "current date" `EDIT-DATE-OF-BIRTH` compares the
/// birth date with (V-COACTUPC-077, P-CSUTLDPY-013).
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// Account 1 of the fixture, as `ACCTSID` holds it (P-CVACT01Y-015); its
/// customer is 000000001 (P-CVCUS01Y-021).
const ACCOUNT_PATH: &str = "/accounts/00000000001";

/// The rejected `Account Status` every later edit row is sent behind.
const STATUS_ROW: &str = "V-COACTUPC-012";

/// The first `CSUTLDTC` row: rows ranked before it are the answer's message.
const FIRST_CSUTLDTC_ROW: &str = "V-COACTUPC-023";

/// The no-message row of a body without a well-formed version token.
const MISSING_VERSION_ROW: &str = "V-COACTUPC-001";

/// The escalation that holds every `CSUTLDPY` `CSUTLDTC` call at `halt`.
const CSUTLDTC_ESCALATION: &str = "E-26";

/// The status every `halt` escalation's Status cell starts with.
const HALT: &str = "halt";

/// The rows the field-edit test leaves to the other tests of this file, each
/// cited where it is exercised, and V-COACTUPC-007, which
/// `account_read_isolation.rs` exercises: the request shape, the key edit and
/// the reads (V-COACTUPC-001 to V-COACTUPC-010), and the rows held behind the
/// E-26 halt.
const ROWS_ELSEWHERE: [&str; 19] = [
    "V-COACTUPC-001",
    "V-COACTUPC-002",
    "V-COACTUPC-003",
    "V-COACTUPC-004",
    "V-COACTUPC-005",
    "V-COACTUPC-006",
    "V-COACTUPC-007",
    "V-COACTUPC-008",
    "V-COACTUPC-009",
    "V-COACTUPC-010",
    "V-COACTUPC-023",
    "V-COACTUPC-036",
    "V-COACTUPC-049",
    "V-COACTUPC-076",
    "V-COACTUPC-124",
    "V-COACTUPC-125",
    "V-COACTUPC-126",
    "V-COACTUPC-127",
    "V-COACTUPC-128",
];

/// The field-edit rows, each sent from its example's inputs: every
/// `COACTUPC` row but [`ROWS_ELSEWHERE`].
const FIELD_EDIT_ROWS: [&str; 109] = [
    "V-COACTUPC-011",
    "V-COACTUPC-012",
    "V-COACTUPC-013",
    "V-COACTUPC-014",
    "V-COACTUPC-015",
    "V-COACTUPC-016",
    "V-COACTUPC-017",
    "V-COACTUPC-018",
    "V-COACTUPC-019",
    "V-COACTUPC-020",
    "V-COACTUPC-021",
    "V-COACTUPC-022",
    "V-COACTUPC-024",
    "V-COACTUPC-025",
    "V-COACTUPC-026",
    "V-COACTUPC-027",
    "V-COACTUPC-028",
    "V-COACTUPC-029",
    "V-COACTUPC-030",
    "V-COACTUPC-031",
    "V-COACTUPC-032",
    "V-COACTUPC-033",
    "V-COACTUPC-034",
    "V-COACTUPC-035",
    "V-COACTUPC-037",
    "V-COACTUPC-038",
    "V-COACTUPC-039",
    "V-COACTUPC-040",
    "V-COACTUPC-041",
    "V-COACTUPC-042",
    "V-COACTUPC-043",
    "V-COACTUPC-044",
    "V-COACTUPC-045",
    "V-COACTUPC-046",
    "V-COACTUPC-047",
    "V-COACTUPC-048",
    "V-COACTUPC-050",
    "V-COACTUPC-051",
    "V-COACTUPC-052",
    "V-COACTUPC-053",
    "V-COACTUPC-054",
    "V-COACTUPC-055",
    "V-COACTUPC-056",
    "V-COACTUPC-057",
    "V-COACTUPC-058",
    "V-COACTUPC-059",
    "V-COACTUPC-060",
    "V-COACTUPC-061",
    "V-COACTUPC-062",
    "V-COACTUPC-063",
    "V-COACTUPC-064",
    "V-COACTUPC-065",
    "V-COACTUPC-066",
    "V-COACTUPC-067",
    "V-COACTUPC-068",
    "V-COACTUPC-069",
    "V-COACTUPC-070",
    "V-COACTUPC-071",
    "V-COACTUPC-072",
    "V-COACTUPC-073",
    "V-COACTUPC-074",
    "V-COACTUPC-075",
    "V-COACTUPC-077",
    "V-COACTUPC-078",
    "V-COACTUPC-079",
    "V-COACTUPC-080",
    "V-COACTUPC-081",
    "V-COACTUPC-082",
    "V-COACTUPC-083",
    "V-COACTUPC-084",
    "V-COACTUPC-085",
    "V-COACTUPC-086",
    "V-COACTUPC-087",
    "V-COACTUPC-088",
    "V-COACTUPC-089",
    "V-COACTUPC-090",
    "V-COACTUPC-091",
    "V-COACTUPC-092",
    "V-COACTUPC-093",
    "V-COACTUPC-094",
    "V-COACTUPC-095",
    "V-COACTUPC-096",
    "V-COACTUPC-097",
    "V-COACTUPC-098",
    "V-COACTUPC-099",
    "V-COACTUPC-100",
    "V-COACTUPC-101",
    "V-COACTUPC-102",
    "V-COACTUPC-103",
    "V-COACTUPC-104",
    "V-COACTUPC-105",
    "V-COACTUPC-106",
    "V-COACTUPC-107",
    "V-COACTUPC-108",
    "V-COACTUPC-109",
    "V-COACTUPC-110",
    "V-COACTUPC-111",
    "V-COACTUPC-112",
    "V-COACTUPC-113",
    "V-COACTUPC-114",
    "V-COACTUPC-115",
    "V-COACTUPC-116",
    "V-COACTUPC-117",
    "V-COACTUPC-118",
    "V-COACTUPC-119",
    "V-COACTUPC-120",
    "V-COACTUPC-121",
    "V-COACTUPC-122",
    "V-COACTUPC-123",
];

/// The `RESP` and `RESP2` E-44 gives a CICS `NOTFND`.
const RESP_NOTFND: (u32, u32) = (13, 80);

/// The `RESP` and `RESP2` E-44 gives a database failure (`IOERR`).
const RESP_IOERR: (u32, u32) = (17, 0);

/// `MOVE 'READ' TO ERROR-OPNAME` of the `WHEN OTHER` branch of each read
/// [app/cbl/COACTUPC.cbl:L3689] [app/cbl/COACTUPC.cbl:L3739]
/// [app/cbl/COACTUPC.cbl:L3788].
const ERROR_OPNAME_READ: &str = "READ";

/// `{WS-EDIT-VARIABLE-NAME}` of a STRING-built message cell.
const PLACEHOLDER: &str = "{WS-EDIT-VARIABLE-NAME}";

/// `WS-EDIT-VARIABLE-NAME PIC X(25)` [app/cbl/COACTUPC.cbl:L53]: a longer
/// label is cut before `FUNCTION TRIM` (E-45).
const EDIT_NAME_LEN: usize = 25;

/// The `WS-EDIT-VARIABLE-NAME` labels of the `COACTUPC` field edits.
const LABELS: [&str; 26] = [
    messages::FIELD_ACCOUNT_STATUS,
    messages::FIELD_OPEN_DATE,
    messages::FIELD_CREDIT_LIMIT,
    messages::FIELD_EXPIRY_DATE,
    messages::FIELD_CASH_CREDIT_LIMIT,
    messages::FIELD_REISSUE_DATE,
    messages::FIELD_CURRENT_BALANCE,
    messages::FIELD_CURRENT_CYCLE_CREDIT_LIMIT,
    messages::FIELD_CURRENT_CYCLE_DEBIT_LIMIT,
    messages::FIELD_SSN_FIRST_3_CHARS,
    messages::FIELD_SSN_4TH_AND_5TH_CHARS,
    messages::FIELD_SSN_LAST_4_CHARS,
    messages::FIELD_DATE_OF_BIRTH,
    messages::FIELD_FICO_SCORE,
    messages::FIELD_FIRST_NAME,
    messages::FIELD_MIDDLE_NAME,
    messages::FIELD_LAST_NAME,
    messages::FIELD_ADDRESS_LINE_1,
    messages::FIELD_STATE,
    messages::FIELD_ZIP,
    messages::FIELD_CITY,
    messages::FIELD_COUNTRY,
    messages::FIELD_PHONE_NUMBER_1,
    messages::FIELD_PHONE_NUMBER_2,
    messages::FIELD_EFT_ACCOUNT_ID,
    messages::FIELD_PRIMARY_CARD_HOLDER,
];

/// The field-edit texts `STRING` puts after the trimmed label.
const SUFFIXES: [&str; 30] = [
    messages::MUST_BE_SUPPLIED,
    messages::MUST_BE_Y_OR_N,
    messages::YEAR_MUST_BE_SUPPLIED,
    messages::MUST_BE_4_DIGIT_NUMBER,
    messages::CENTURY_NOT_VALID,
    messages::MONTH_MUST_BE_SUPPLIED,
    messages::MONTH_MUST_BE_A_NUMBER_BETWEEN_1_AND_12,
    messages::DAY_MUST_BE_SUPPLIED,
    messages::DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31,
    messages::CANNOT_HAVE_31_DAYS_IN_THIS_MONTH,
    messages::CANNOT_HAVE_30_DAYS_IN_THIS_MONTH,
    messages::NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS,
    messages::CANNOT_BE_IN_THE_FUTURE,
    messages::IS_NOT_VALID,
    messages::MUST_BE_ALL_NUMERIC,
    messages::MUST_NOT_BE_ZERO,
    messages::SHOULD_NOT_BE_000_666_OR_BETWEEN_900_AND_999,
    messages::SHOULD_BE_BETWEEN_300_AND_850,
    messages::CAN_HAVE_ALPHABETS_ONLY,
    messages::IS_NOT_A_VALID_STATE_CODE,
    messages::AREA_CODE_MUST_BE_SUPPLIED,
    messages::AREA_CODE_MUST_BE_A_3_DIGIT_NUMBER,
    messages::AREA_CODE_CANNOT_BE_ZERO,
    messages::NOT_VALID_NORTH_AMERICA_AREA_CODE,
    messages::PREFIX_CODE_MUST_BE_SUPPLIED,
    messages::PREFIX_CODE_MUST_BE_A_3_DIGIT_NUMBER,
    messages::PREFIX_CODE_CANNOT_BE_ZERO,
    messages::LINE_NUMBER_CODE_MUST_BE_SUPPLIED,
    messages::LINE_NUMBER_CODE_MUST_BE_A_4_DIGIT_NUMBER,
    messages::LINE_NUMBER_CODE_CANNOT_BE_ZERO,
];

/// The `COACTUPC` texts a row quotes whole.
const WHOLE_TEXTS: [&str; 8] = [
    messages::NO_INPUT_RECEIVED,
    messages::ACCOUNT_NUMBER_IF_SUPPLIED_MUST_BE_11_DIGIT_NON_ZERO,
    messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED,
    messages::INVALID_ZIP_CODE_FOR_STATE,
    messages::COULD_NOT_LOCK_ACCOUNT_RECORD_FOR_UPDATE,
    messages::COULD_NOT_LOCK_CUSTOMER_RECORD_FOR_UPDATE,
    messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
    messages::UPDATE_OF_RECORD_FAILED,
];

/// Every `COACTUP` input field of the body, in map order: (BMS field, JSON
/// name of `AccountUpdateRequest`) [app/cpy-bms/COACTUP.CPY].
const BODY_FIELDS: [(&str, &str); 42] = [
    ("ACSTTUS", "acct_active_status"),
    ("OPNYEAR", "acct_open_year"),
    ("OPNMON", "acct_open_mon"),
    ("OPNDAY", "acct_open_day"),
    ("ACRDLIM", "acct_credit_limit"),
    ("EXPYEAR", "acct_exp_year"),
    ("EXPMON", "acct_exp_mon"),
    ("EXPDAY", "acct_exp_day"),
    ("ACSHLIM", "acct_cash_credit_limit"),
    ("RISYEAR", "acct_reissue_year"),
    ("RISMON", "acct_reissue_mon"),
    ("RISDAY", "acct_reissue_day"),
    ("ACURBAL", "acct_curr_bal"),
    ("ACRCYCR", "acct_curr_cyc_credit"),
    ("AADDGRP", "acct_group_id"),
    ("ACRCYDB", "acct_curr_cyc_debit"),
    ("ACSTNUM", "customer_id"),
    ("ACTSSN1", "cust_ssn_1"),
    ("ACTSSN2", "cust_ssn_2"),
    ("ACTSSN3", "cust_ssn_3"),
    ("DOBYEAR", "cust_dob_year"),
    ("DOBMON", "cust_dob_mon"),
    ("DOBDAY", "cust_dob_day"),
    ("ACSTFCO", "cust_fico_credit_score"),
    ("ACSFNAM", "cust_first_name"),
    ("ACSMNAM", "cust_middle_name"),
    ("ACSLNAM", "cust_last_name"),
    ("ACSADL1", "cust_addr_line_1"),
    ("ACSSTTE", "cust_addr_state_cd"),
    ("ACSADL2", "cust_addr_line_2"),
    ("ACSZIPC", "cust_addr_zip"),
    ("ACSCITY", "cust_addr_line_3"),
    ("ACSCTRY", "cust_addr_country_cd"),
    ("ACSPH1A", "cust_phone_num_1a"),
    ("ACSPH1B", "cust_phone_num_1b"),
    ("ACSPH1C", "cust_phone_num_1c"),
    ("ACSGOVT", "cust_govt_issued_id"),
    ("ACSPH2A", "cust_phone_num_2a"),
    ("ACSPH2B", "cust_phone_num_2b"),
    ("ACSPH2C", "cust_phone_num_2c"),
    ("ACSEFTC", "cust_eft_account_id"),
    ("ACSPFLG", "cust_pri_card_holder_ind"),
];

/// The body fields that repeat a fetched field of the same name, the version
/// token included.
const COPIED_FIELDS: [&str; 21] = [
    "acct_active_status",
    "acct_credit_limit",
    "acct_cash_credit_limit",
    "acct_curr_bal",
    "acct_curr_cyc_credit",
    "acct_curr_cyc_debit",
    "acct_group_id",
    "customer_id",
    "cust_fico_credit_score",
    "cust_first_name",
    "cust_middle_name",
    "cust_last_name",
    "cust_addr_line_1",
    "cust_addr_state_cd",
    "cust_addr_line_2",
    "cust_addr_zip",
    "cust_addr_line_3",
    "cust_addr_country_cd",
    "cust_govt_issued_id",
    "cust_eft_account_id",
    "cust_pri_card_holder_ind",
];

/// The body fields that are a part of a fetched field, as the `COACTUP` map
/// splits it: (body field, fetched field, first byte, end byte). Dates are
/// `YYYY-MM-DD`, the SSN is 9 digits and phones are `(aaa)bbb-cccc`
/// (P-COACTUPC-027, P-COACTUPC-031).
const SPLIT_FIELDS: [(&str, &str, usize, usize); 21] = [
    ("acct_open_year", "acct_open_date", 0, 4),
    ("acct_open_mon", "acct_open_date", 5, 7),
    ("acct_open_day", "acct_open_date", 8, 10),
    ("acct_exp_year", "acct_expiraion_date", 0, 4),
    ("acct_exp_mon", "acct_expiraion_date", 5, 7),
    ("acct_exp_day", "acct_expiraion_date", 8, 10),
    ("acct_reissue_year", "acct_reissue_date", 0, 4),
    ("acct_reissue_mon", "acct_reissue_date", 5, 7),
    ("acct_reissue_day", "acct_reissue_date", 8, 10),
    ("cust_ssn_1", "cust_ssn", 0, 3),
    ("cust_ssn_2", "cust_ssn", 3, 5),
    ("cust_ssn_3", "cust_ssn", 5, 9),
    ("cust_dob_year", "cust_dob_yyyy_mm_dd", 0, 4),
    ("cust_dob_mon", "cust_dob_yyyy_mm_dd", 5, 7),
    ("cust_dob_day", "cust_dob_yyyy_mm_dd", 8, 10),
    ("cust_phone_num_1a", "cust_phone_num_1", 1, 4),
    ("cust_phone_num_1b", "cust_phone_num_1", 5, 8),
    ("cust_phone_num_1c", "cust_phone_num_1", 9, 13),
    ("cust_phone_num_2a", "cust_phone_num_2", 1, 4),
    ("cust_phone_num_2b", "cust_phone_num_2", 5, 8),
    ("cust_phone_num_2c", "cust_phone_num_2", 9, 13),
];

/// The three fetched values of customer 000000001 that fail their own edit,
/// each replaced by a value that passes, so that a request changes only what
/// its case names: FICO 274 is out of range (V-COACTUPC-081), area code 373
/// is no general purpose code (V-COACTUPC-111, P-CSLKPCDY-002), and `NC`
/// with a zip starting `12` is no state and zip pair (V-COACTUPC-123,
/// P-CSLKPCDY-005), while `NC27` is.
const PASSING_VALUES: [(&str, &str); 3] = [
    ("cust_fico_credit_score", "700"),
    ("cust_phone_num_2a", "908"),
    ("cust_addr_zip", "27546"),
];

/// A change of one account field and one customer field that passes every
/// edit: the credit limit (`ACRDLIM`) and the first name (`ACSFNAM`).
const VALID_CHANGE: [(&str, &str); 2] = [
    ("acct_credit_limit", "2525.00"),
    ("cust_first_name", "Jane"),
];

/// A credit limit whose `NUMVAL-C` value is a midpoint beyond the cent, an
/// input class E-40 holds at `halt` (P-COACTUPC-049).
const MIDPOINT_CREDIT_LIMIT: &str = "2020.005";

/// The row whose example names its birth date only as "equal to the pinned
/// clock date": [`clock_birth_date`] gives its input.
const CLOCK_BIRTH_DATE_ROW: &str = "V-COACTUPC-077";

/// The request shape check before any COBOL edit: a token that is not 16
/// lower-case hex digits (V-COACTUPC-001).
const MALFORMED_VERSION: &str = "not-a-version";

/// Account 2 loses its master record while its cross-reference stays
/// (V-COACTUPC-006); `card_xref` has no foreign key, as `CXACAIX` has none.
const DELETE_ACCOUNT_2: &str = "DELETE FROM accounts WHERE account_id = 2";

/// The account number of [`DELETE_ACCOUNT_2`], as `ACCTSID` holds it.
const ACCOUNT_2: &str = "00000000002";

/// The customer the cross-reference of account 3 names loses its master
/// record while the cross-reference stays (V-COACTUPC-008). The statement
/// follows the seeded `card_xref` row of the account rather than a customer
/// number, so the customer it removes is the one the test decodes from the
/// `cardxref.txt` record of [`ACCOUNT_3`].
const DELETE_CUSTOMER_OF_ACCOUNT_3: &str = "DELETE FROM customers WHERE customer_id IN \
     (SELECT xref_cust_id FROM card_xref WHERE xref_acct_id = 3)";

/// The account number whose customer [`DELETE_CUSTOMER_OF_ACCOUNT_3`]
/// removes, as `ACCTSID` holds it.
const ACCOUNT_3: &str = "00000000003";

/// The column of a decoded cross-reference holding its customer,
/// `XREF-CUST-ID 9(09)` (P-CVACT03Y-002): the nine digits
/// `WS-CARD-RID-CUST-ID-X` holds.
const XREF_CUSTOMER: &str = "xref_cust_id";

/// Hides the cross-reference: its read then fails with SQLSTATE `42P01`
/// (V-COACTUPC-005).
const HIDE_XREF: &str = "ALTER TABLE card_xref RENAME TO card_xref_offline";

/// Puts the cross-reference back.
const RESTORE_XREF: &str = "ALTER TABLE card_xref_offline RENAME TO card_xref";

/// Hides the customer master: its read then fails (V-COACTUPC-009).
const HIDE_CUSTOMERS: &str = "ALTER TABLE customers RENAME TO customers_offline";

/// Puts the customer master back.
const RESTORE_CUSTOMERS: &str = "ALTER TABLE customers_offline RENAME TO customers";

/// Another writer changes a compared account field of account 1 between the
/// `GET` and the `PUT` (P-COACTUPC-012).
const ACCOUNT_CHANGED_ELSEWHERE: &str =
    "UPDATE accounts SET acct_credit_limit = acct_credit_limit + 1 WHERE account_id = 1";

/// Another writer changes a compared customer field of customer 000000001.
const CUSTOMER_CHANGED_ELSEWHERE: &str =
    "UPDATE customers SET cust_first_name = 'Elsewhere' WHERE customer_id = 1";

/// The first name [`CUSTOMER_CHANGED_ELSEWHERE`] stores.
const FIRST_NAME_CHANGED_ELSEWHERE: &str = "Elsewhere";

/// The section D row recording the seeded state of account 1, its credit
/// limit included.
const SEEDED_ACCOUNT_ROW: &str = "P-CVACT01Y-015";

/// A test-only trigger function that fails every customer rewrite, as a
/// `CUSTDAT` `REWRITE` that does not return `NORMAL` (V-COACTUPC-128).
const CREATE_FAILING_FUNCTION: &str = "CREATE FUNCTION test_fail_customer_update() \
     RETURNS trigger LANGUAGE plpgsql AS $$ \
     BEGIN RAISE EXCEPTION 'test-only customer failure'; END $$";

/// Installs [`CREATE_FAILING_FUNCTION`] before every update of `customers`.
const CREATE_FAILING_TRIGGER: &str = "CREATE TRIGGER test_fail_customer_update \
     BEFORE UPDATE ON customers FOR EACH ROW EXECUTE FUNCTION test_fail_customer_update()";

/// The trigger [`CREATE_FAILING_TRIGGER`] installs, as `pg_trigger` names it.
const FAILING_TRIGGER: &str =
    "SELECT tgname::text FROM pg_trigger WHERE tgname = 'test_fail_customer_update'";

/// The text the failing trigger raises, which no answer may carry.
const TRIGGER_FAILURE_TEXT: &str = "test-only customer failure";

/// The account and customer fields [`VALID_CHANGE`] sets and the writers
/// above change, as stored.
const CHANGED_FIELDS_ROW: &str = "SELECT a.acct_credit_limit::text, c.cust_first_name \
     FROM accounts a, customers c WHERE a.account_id = 1 AND c.customer_id = 1";

/// One `COACTUPC` row of the validation audit, as these tests read it.
struct AuditRow {
    /// The `V-COACTUPC-nnn` ID.
    id: String,
    /// The HTTP status of the answer.
    status: StatusCode,
    /// The `kind=` of the trigger condition; `None` for `kind=none`.
    kind: Option<ErrorKind>,
    /// The message text between its quotes; `None` for a `no-message` row.
    template: Option<String>,
    /// The whole message cell, notes included.
    message_cell: String,
    /// The label the message note names (`(name '...')`), as cut to 25 bytes.
    name: Option<String>,
    /// Whether the validation paragraph cites `CSUTLDPY`, the date edits.
    csutldpy: bool,
    /// The BMS field the answer's first `fields` entry names.
    field: String,
    /// The rank at which `COACTUPC` reaches the rejection.
    order: u32,
    /// The `example=` of the trigger condition.
    example: String,
}

/// The row `id` of the validation audit `doc`.
fn audit_row(doc: &Document, id: &str) -> AuditRow {
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
    let trigger = key_values(cell("Trigger condition"));
    let value = |key: &str| {
        trigger.iter().find(|pair| pair.key == key).map_or_else(
            || panic!("{id} has no `{key}=` in its trigger condition"),
            |pair| pair.value.clone(),
        )
    };
    let message_cell = cell("Message text").to_owned();
    let template = (!is_no_message(&message_cell)).then(|| quoted_text(&message_cell, id));
    let name = message_cell
        .split_once("(name '")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(name, _)| name.to_owned());
    AuditRow {
        id: id.to_owned(),
        status,
        kind: kind_of(&value("kind"), id),
        template,
        name,
        csutldpy: citations(cell("Validation paragraph"))
            .iter()
            .any(|citation| citation.path.ends_with("CSUTLDPY.cpy")),
        field: cell("BMS field").to_owned(),
        order: cell("Order")
            .parse()
            .unwrap_or_else(|_| panic!("{id} has no numeric Order: {}", cell("Order"))),
        example: value("example"),
        message_cell,
    }
}

/// The `DomainError` kind a trigger condition's `kind=` names, `None` for
/// `none`, the message-only outcome.
fn kind_of(kind: &str, id: &str) -> Option<ErrorKind> {
    match kind {
        "Invalid" => Some(ErrorKind::Invalid),
        "Empty" => Some(ErrorKind::Empty),
        "NotFound" => Some(ErrorKind::NotFound),
        "Duplicate" => Some(ErrorKind::Duplicate),
        "Conflict" => Some(ErrorKind::Conflict),
        "SignonFailed" => Some(ErrorKind::SignonFailed),
        "Unexpected" => Some(ErrorKind::Unexpected),
        "none" => None,
        other => panic!("{id} names an unknown kind `{other}`"),
    }
}

/// The text of the first code span of a message cell, without the double
/// quotes around it, `\"` and `\\` read as the characters they stand for.
fn quoted_text(cell: &str, id: &str) -> String {
    let span = code_span(cell).unwrap_or_else(|| panic!("{id} quotes no message text: {cell}"));
    let inner = span
        .text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id} message is not a double-quoted string: {cell}"));
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        text.push(if ch == '\\' {
            chars.next().unwrap_or(ch)
        } else {
            ch
        });
    }
    text
}

/// The `carddemo_domain::messages` text of `row`, checked against the text
/// the row quotes.
///
/// A STRING-built row, `{WS-EDIT-VARIABLE-NAME}` followed by a field-edit
/// text, is composed from the `FIELD_*` label its note names and the
/// catalog's field-edit text: through `messages::date_edit_message` for a
/// `CSUTLDPY` date edit, `messages::field_edit_message` otherwise. Any other
/// row quotes one catalog text whole.
fn catalog_text(row: &AuditRow) -> String {
    let id = &row.id;
    let template = row
        .template
        .as_deref()
        .unwrap_or_else(|| panic!("{id} records no message"));
    let Some(tail) = template.strip_prefix(PLACEHOLDER) else {
        return WHOLE_TEXTS
            .iter()
            .find(|text| **text == template)
            .map_or_else(
                || panic!("{id}: no catalog text is {template:?}"),
                |text| (*text).to_owned(),
            );
    };
    let name = row
        .name
        .as_deref()
        .unwrap_or_else(|| panic!("{id} names no field label: {}", row.message_cell));
    let label = LABELS
        .into_iter()
        .find(|label| label.get(..EDIT_NAME_LEN).unwrap_or(*label).trim() == name)
        .unwrap_or_else(|| panic!("{id}: no catalog label is {name:?}"));
    let suffix = SUFFIXES
        .iter()
        .find(|suffix| **suffix == tail)
        .unwrap_or_else(|| panic!("{id}: no catalog field-edit text is {tail:?}"));
    let text = if row.csutldpy {
        messages::date_edit_message(label, suffix)
    } else {
        messages::field_edit_message(label, suffix)
    };
    assert_eq!(
        text,
        template.replacen(PLACEHOLDER, name, 1),
        "{id}: the catalog text differs from the audited one"
    );
    text
}

/// The `ERROR-FILE` literal a file-error row's message cell names: the
/// quoted text after its last `ERROR-FILE`, for example `CXACAIX `.
fn error_file(row: &AuditRow) -> &str {
    row.message_cell
        .rsplit_once("ERROR-FILE")
        .and_then(|(_, after)| after.split_once('\''))
        .and_then(|(_, quoted)| quoted.split_once('\''))
        .map_or_else(
            || panic!("{}: no quoted ERROR-FILE: {}", row.id, row.message_cell),
            |(file, _)| file,
        )
}

/// The JSON body field of the `COACTUP` input field `bms`.
fn json_name(bms: &str) -> Option<&'static str> {
    BODY_FIELDS
        .iter()
        .find(|(field, _)| *field == bms)
        .map(|(_, json)| *json)
}

/// The failing input a row's example names: each BMS input field with a
/// quoted value, as its JSON body field, and every value the example offers
/// for it, the first one first (`ACSTTUS "y" or "X"` gives two). Quoted
/// values of no body field, such as an account number, are not inputs.
fn example_inputs(row: &AuditRow) -> Vec<(&'static str, Vec<String>)> {
    let mut inputs: Vec<(&'static str, Vec<String>)> = Vec::new();
    for (word, value) in quoted_values(row) {
        if let Some(json) = json_name(word) {
            inputs.push((json, vec![value.to_owned()]));
        } else if word == "or" {
            if let Some((_, values)) = inputs.last_mut() {
                values.push(value.to_owned());
            }
        }
    }
    inputs
}

/// Each double-quoted value of a row's example with the word before it, for
/// example `("ACSTTUS", "y")` and `("or", "X")` of `ACSTTUS "y" or "X"`.
fn quoted_values(row: &AuditRow) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    let mut rest = row.example.as_str();
    while let Some((before, after)) = rest.split_once('"') {
        let (value, tail) = after
            .split_once('"')
            .unwrap_or_else(|| panic!("{}: unbalanced quote in {:?}", row.id, row.example));
        let word = before
            .trim_end()
            .rsplit(|ch: char| !ch.is_ascii_alphanumeric())
            .next()
            .unwrap_or_default();
        found.push((word, value));
        rest = tail;
    }
    found
}

/// The path of a row's example `PUT <path> ...`.
fn example_path(row: &AuditRow) -> &str {
    row.example
        .strip_prefix("PUT ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("{}: the example names no PUT path: {}", row.id, row.example))
}

/// The 11-digit account number a row's example names.
fn example_account(row: &AuditRow) -> &str {
    row.example
        .split_whitespace()
        .find(|word| word.len() == 11 && word.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or_else(|| panic!("{}: the example names no account: {}", row.id, row.example))
}

/// The worked text a message note quotes after `gives`, for example the
/// `NOTFND` text of account 99999999999.
fn worked_text(row: &AuditRow) -> &str {
    row.message_cell
        .split_once("gives '")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map_or_else(
            || panic!("{}: the note quotes no worked text", row.id),
            |(text, _)| text,
        )
}

/// The input of [`CLOCK_BIRTH_DATE_ROW`]: a birth date equal to the date of
/// [`CLOCK`], which `EDIT-DATE-OF-BIRTH` rejects (P-CSUTLDPY-013).
fn clock_birth_date() -> Vec<(&'static str, Vec<String>)> {
    let part = |from: usize, to: usize| {
        CLOCK
            .get(from..to)
            .unwrap_or_else(|| panic!("{CLOCK} has no bytes {from}..{to}"))
            .to_owned()
    };
    vec![
        ("cust_dob_year", vec![part(0, 4)]),
        ("cust_dob_mon", vec![part(5, 7)]),
        ("cust_dob_day", vec![part(8, 10)]),
    ]
}

/// One request's changes to the body: (JSON body field, value).
type Case = Vec<(&'static str, String)>;

/// The requests of one row: the first value of every input, then each other
/// value of an input with the others at their first.
fn cases(inputs: &[(&'static str, Vec<String>)]) -> Vec<Case> {
    let first: Case = inputs
        .iter()
        .map(|(name, values)| (*name, values.first().cloned().unwrap_or_default()))
        .collect();
    let mut all = vec![first.clone()];
    for (index, (name, values)) in inputs.iter().enumerate() {
        for alternative in values.iter().skip(1) {
            let mut case = first.clone();
            case[index] = (*name, alternative.clone());
            all.push(case);
        }
    }
    all
}

/// A field-edit row with its catalog text and its requests.
struct EditRow {
    row: AuditRow,
    text: String,
    cases: Vec<Case>,
}

/// The field-edit rows of `COACTUPC` and the rejected status they are sent
/// behind.
struct EditPlan {
    /// [`STATUS_ROW`], its catalog text and its first failing input, as
    /// (JSON body field, value).
    status_row: AuditRow,
    status_text: String,
    status_input: (&'static str, String),
    /// The Order of [`FIRST_CSUTLDTC_ROW`]: rows ranked before it are the
    /// answer's message.
    csutldtc_order: u32,
    /// Every row whose example names body inputs, in audit order.
    rows: Vec<EditRow>,
}

impl EditPlan {
    /// The field-edit row `id`.
    fn row(&self, id: &str) -> &EditRow {
        self.rows
            .iter()
            .find(|edit| edit.row.id == id)
            .unwrap_or_else(|| panic!("{id} is no field-edit row"))
    }
}

/// The plan of every `COACTUPC` row of `doc`.
///
/// # Panics
///
/// Panics unless the rows without body inputs are exactly
/// [`ROWS_ELSEWHERE`], so that every `COACTUPC` row, `no-message` rows
/// included, is exercised by this file or by the test it names.
fn edit_plan(doc: &Document) -> EditPlan {
    let status_row = audit_row(doc, STATUS_ROW);
    let status_text = catalog_text(&status_row);
    let status_input = status_input(&status_row);
    let csutldtc_order = audit_row(doc, FIRST_CSUTLDTC_ROW).order;
    let mut rows = Vec::new();
    let mut without_inputs = BTreeSet::new();
    for id in validation_ids(PROGRAM) {
        let row = audit_row(doc, &id);
        let inputs = if id == CLOCK_BIRTH_DATE_ROW {
            clock_birth_date()
        } else {
            example_inputs(&row)
        };
        if inputs.is_empty() || row.template.is_none() {
            without_inputs.insert(id);
            continue;
        }
        let text = catalog_text(&row);
        rows.push(EditRow {
            cases: cases(&inputs),
            text,
            row,
        });
    }
    let listed =
        |ids: &[&str]| -> BTreeSet<String> { ids.iter().map(|id| (*id).to_owned()).collect() };
    assert_eq!(
        without_inputs,
        listed(&ROWS_ELSEWHERE),
        "every {PROGRAM} row is a field-edit row here or is exercised where ROWS_ELSEWHERE says"
    );
    let sent: BTreeSet<String> = rows.iter().map(|edit| edit.row.id.clone()).collect();
    assert_eq!(
        sent,
        listed(&FIELD_EDIT_ROWS),
        "the field-edit rows of the audit are the rows FIELD_EDIT_ROWS cites"
    );
    EditPlan {
        status_row,
        status_text,
        status_input,
        csutldtc_order,
        rows,
    }
}

/// The first failing input the example of [`STATUS_ROW`] names.
fn status_input(status_row: &AuditRow) -> (&'static str, String) {
    example_inputs(status_row)
        .into_iter()
        .next()
        .and_then(|(name, values)| values.into_iter().next().map(|value| (name, value)))
        .unwrap_or_else(|| panic!("{STATUS_ROW} names no failing status"))
}

/// The string `name` of the fetched record.
fn fetched_text<'a>(fetched: &'a Value, name: &str) -> &'a str {
    fetched[name]
        .as_str()
        .unwrap_or_else(|| panic!("GET answered no string `{name}`: {fetched}"))
}

/// The bytes `from..to` of the fetched string `name`.
fn part<'a>(fetched: &'a Value, name: &str, from: usize, to: usize) -> &'a str {
    let value = fetched_text(fetched, name);
    value
        .get(from..to)
        .unwrap_or_else(|| panic!("`{name}` {value:?} has no bytes {from}..{to}"))
}

/// The `PUT` body that repeats the fetched record as the `COACTUP` fields
/// show it, every field exactly as fetched, with its version token.
fn update_body(fetched: &Value) -> Map<String, Value> {
    let copied = COPIED_FIELDS
        .iter()
        .map(|name| (*name, fetched_text(fetched, name)));
    let split = SPLIT_FIELDS
        .iter()
        .map(|(name, source, from, to)| (*name, part(fetched, source, *from, *to)));
    copied
        .chain(split)
        .chain([("version", fetched_text(fetched, "version"))])
        .map(|(name, value)| (name.to_owned(), Value::String(value.to_owned())))
        .collect()
}

/// [`update_body`] with [`PASSING_VALUES`]: a change every edit passes.
fn passing_body(fetched: &Value) -> Map<String, Value> {
    let mut body = update_body(fetched);
    set(&mut body, &PASSING_VALUES);
    body
}

/// Sets each `(field, value)` of `changes` in `body`.
fn set(body: &mut Map<String, Value>, changes: &[(&str, &str)]) {
    for (name, value) in changes {
        body.insert((*name).to_owned(), Value::String((*value).to_owned()));
    }
}

/// `body` with each `(field, value)` of `changes` set.
fn changed(body: &Map<String, Value>, changes: &[(&str, &str)]) -> Value {
    let mut body = body.clone();
    set(&mut body, changes);
    Value::Object(body)
}

/// `body` without its field `name`.
fn without(body: &Map<String, Value>, name: &str) -> Value {
    let mut body = body.clone();
    body.remove(name);
    Value::Object(body)
}

/// `GET /accounts/00000000001`, which must answer 200.
async fn fetch(app: &TestApp, token: &str) -> Value {
    let (status, fetched, raw) = app.get(ACCOUNT_PATH, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {ACCOUNT_PATH}: {raw}");
    fetched
}

/// The `(field, message)` entries of an error answer's `fields`, in order.
fn entries(resp: &Resp) -> Vec<(String, String)> {
    let (_, body, raw) = resp;
    let entry = |item: &Value, key: &str| {
        item[key]
            .as_str()
            .unwrap_or_else(|| panic!("a `fields` entry has no string `{key}`: {raw}"))
            .to_owned()
    };
    match &body["fields"] {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .map(|item| (entry(item, "field"), entry(item, "message")))
            .collect(),
        other => panic!("`fields` is not an array but {other}: {raw}"),
    }
}

/// The `code` of an answer of `row`'s kind.
fn code(row: &AuditRow) -> &'static str {
    row.kind
        .map_or_else(|| panic!("{} has no error kind", row.id), ErrorKind::code)
}

/// Asserts that `resp` is `row`'s own answer: its status, its kind's code,
/// `text` as the message, and `fields` led by the row's BMS field, every
/// entry carrying `text` (the fields its edit flagged).
fn assert_row_answer(resp: &Resp, row: &AuditRow, text: &str, case: &str) {
    let (status, body, shown) = resp;
    assert_eq!(*status, row.status, "{case}: status: {shown}");
    assert_eq!(
        body["code"].as_str(),
        Some(code(row)),
        "{case}: code: {shown}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(text),
        "{case}: message: {shown}"
    );
    let listed = entries(resp);
    assert_eq!(
        listed.first().map(|(field, _)| field.as_str()),
        Some(row.field.as_str()),
        "{case}: the cursor field: {shown}"
    );
    assert!(
        listed.iter().all(|(_, message)| message == text),
        "{case}: every flagged field carries the row's text: {shown}"
    );
}

/// Asserts that `resp` is the rejected status's answer with `edit`'s own
/// entries after the status entry: the status row's status, code and text,
/// `fields` led by `ACSTTUS`, then the fields `edit` flagged, led by its BMS
/// field, each carrying its text.
fn assert_behind_status(resp: &Resp, plan: &EditPlan, edit: &EditRow, case: &str) {
    let status_row = &plan.status_row;
    let (status, body, raw) = resp;
    assert_eq!(
        edit.row.status, status_row.status,
        "{case}: an input edit row shares the status of the edit it follows"
    );
    assert_eq!(*status, status_row.status, "{case}: status: {raw}");
    assert_eq!(
        body["code"].as_str(),
        Some(code(status_row)),
        "{case}: code: {raw}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(plan.status_text.as_str()),
        "{case}: the status's text stays the message: {raw}"
    );
    let listed = entries(resp);
    let (first, own) = listed
        .split_first()
        .unwrap_or_else(|| panic!("{case}: no `fields`: {raw}"));
    assert_eq!(
        first,
        &(status_row.field.clone(), plan.status_text.clone()),
        "{case}: the status entry comes first: {raw}"
    );
    assert_eq!(
        own.first().map(|(field, _)| field.as_str()),
        Some(edit.row.field.as_str()),
        "{case}: the row's cursor field follows the status: {raw}"
    );
    assert!(
        own.iter().all(|(_, message)| *message == edit.text),
        "{case}: every field the row flagged carries its text: {raw}"
    );
}

/// Asserts that `resp` is the answer of an input class held at `halt`: 500,
/// the `Unexpected` status of the audit's status mapping, with
/// `messages::UNEXPECTED_ERROR` and no fields.
fn assert_halt(resp: &Resp, case: &str) {
    assert_error(
        resp,
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorKind::Unexpected.code(),
        messages::UNEXPECTED_ERROR,
        &[],
    );
    assert!(
        !resp.2.contains(TRIGGER_FAILURE_TEXT),
        "{case}: the answer carries internal detail: {}",
        resp.2
    );
}

/// Asserts that the escalation log still holds [`CSUTLDTC_ESCALATION`] at
/// `halt`: the halt answers pinned below depend on it.
fn assert_csutldtc_halt() {
    let log = Repo::new(fixtures::repo_root())
        .escalations()
        .unwrap_or_else(|error| panic!("the escalation log cannot be read: {error}"));
    let (table, row) = log
        .find_row(CSUTLDTC_ESCALATION)
        .unwrap_or_else(|| panic!("the escalation log has no {CSUTLDTC_ESCALATION} row"));
    let status = table.cell(row, "Status").unwrap_or_default().trim();
    assert!(
        status.starts_with(HALT),
        "{CSUTLDTC_ESCALATION} is `{status}`, not {HALT}: the halt answer these tests pin no \
         longer applies to a change that passes every edit"
    );
}

/// COACTUPC 1100-RECEIVE-MAP, 1200-EDIT-MAP-INPUTS with CSUTLDPY and
/// CSLKPCDY — P-COACTUPC-006, P-COACTUPC-007, P-COACTUPC-019,
/// P-COACTUPC-020, P-COACTUPC-021, P-COACTUPC-022, P-COACTUPC-023,
/// P-COACTUPC-024, P-COACTUPC-025, P-COACTUPC-026, P-COACTUPC-027,
/// P-COACTUPC-028, P-COACTUPC-029, P-COACTUPC-030, P-COACTUPC-031,
/// P-COACTUPC-032, P-COACTUPC-033, P-COACTUPC-047, P-CSUTLDPY-001 to
/// P-CSUTLDPY-013, P-CSLKPCDY-002, P-CSLKPCDY-004, P-CSLKPCDY-005, and the
/// rows of [`FIELD_EDIT_ROWS`].
///
/// Each case changes only its row's inputs of the passing body, where the
/// fetched amounts convert exactly (P-COACTUPC-047). P-COACTUPC-023 is the
/// pair of alphanumeric edits no paragraph performs: no case reaches them,
/// and the address line 2 and government ID they would edit stay as fetched.
/// The rows, their inputs and their expected answers are read from the
/// audit, so a row added there is sent here once [`FIELD_EDIT_ROWS`] cites
/// it.
#[test]
fn every_field_edit_row_answers_its_text_at_its_field() {
    let plan = edit_plan(&validation_audit());
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        let fetched = fetch(&app, &token).await;
        let base = passing_body(&fetched);
        for edit in &plan.rows {
            send_row(&app, &token, &base, &plan, edit).await;
        }
        first_failing_edit_wins(&app, &token, &base, &plan).await;
        century_and_leap_years(&app, &token, &base, &plan).await;
        assert_eq!(
            fetch(&app, &token).await,
            fetched,
            "a rejected change must write nothing"
        );
    });
}

/// The changes of `case`, behind the rejected status when `behind_status`.
fn case_changes<'a>(
    plan: &'a EditPlan,
    case: &'a Case,
    behind_status: bool,
) -> Vec<(&'a str, &'a str)> {
    let status = (plan.status_input.0, plan.status_input.1.as_str());
    behind_status
        .then_some(status)
        .into_iter()
        .chain(case.iter().map(|(name, value)| (*name, value.as_str())))
        .collect()
}

/// Sends every case of `edit` on `base`: as the answer's own message when the
/// row ranks before the first `CSUTLDTC` row, behind the rejected status
/// otherwise.
async fn send_row(
    app: &TestApp,
    token: &str,
    base: &Map<String, Value>,
    plan: &EditPlan,
    edit: &EditRow,
) {
    let own_message = edit.row.order < plan.csutldtc_order;
    for case in &edit.cases {
        let changes = case_changes(plan, case, !own_message);
        let label = format!("{} {changes:?}", edit.row.id);
        let resp = app
            .put(ACCOUNT_PATH, Some(token), &changed(base, &changes))
            .await;
        if own_message {
            assert_row_answer(&resp, &edit.row, &edit.text, &label);
        } else {
            assert_behind_status(&resp, plan, edit, &label);
        }
    }
}

/// COACTUPC 1200-EDIT-MAP-INPUTS precedence — P-COACTUPC-006,
/// V-COACTUPC-015, V-COACTUPC-025, V-COACTUPC-079, V-COACTUPC-092
///
/// Two failing edits in one body: the lower Order supplies the message and
/// the cursor, and `fields` lists the flagged fields in Order. The Open Date
/// century (Order 12) wins over the Credit Limit text (Order 22); behind the
/// rejected status, the FICO Score entry (Order 76) comes before the Zip
/// entry (Order 89).
async fn first_failing_edit_wins(
    app: &TestApp,
    token: &str,
    base: &Map<String, Value>,
    plan: &EditPlan,
) {
    let pairs = [
        ("V-COACTUPC-015", "V-COACTUPC-025"),
        ("V-COACTUPC-079", "V-COACTUPC-092"),
    ];
    for (earlier_id, later_id) in pairs {
        let (earlier, later) = (plan.row(earlier_id), plan.row(later_id));
        assert!(
            earlier.row.order < later.row.order,
            "{earlier_id} must rank before {later_id}"
        );
        let behind_status = earlier.row.order >= plan.csutldtc_order;
        let mut changes = case_changes(plan, &earlier.cases[0], behind_status);
        changes.extend(
            later.cases[0]
                .iter()
                .map(|(name, value)| (*name, value.as_str())),
        );
        let resp = app
            .put(ACCOUNT_PATH, Some(token), &changed(base, &changes))
            .await;
        let lead = if behind_status {
            (&plan.status_row, plan.status_text.as_str())
        } else {
            (&earlier.row, earlier.text.as_str())
        };
        let mut expected = Vec::new();
        if behind_status {
            expected.push((plan.status_row.field.as_str(), plan.status_text.as_str()));
        }
        expected.push((earlier.row.field.as_str(), earlier.text.as_str()));
        expected.push((later.row.field.as_str(), later.text.as_str()));
        assert_error(&resp, lead.0.status, code(lead.0), lead.1, &expected);
    }
}

/// An Open Date sent behind the rejected status: (year, month, day, the
/// `(field, message)` entries it adds after the status entry).
type DateCase<'a> = (&'a str, &'a str, &'a str, Vec<(&'a str, &'a str)>);

/// CSUTLDPY EDIT-YEAR-CCYY and EDIT-DAY-MONTH-YEAR — P-CSUTLDPY-004,
/// P-CSUTLDPY-011, P-CSUTLDPY-014, V-COACTUPC-015, V-COACTUPC-022
///
/// Behind the rejected status, whose text pins the `CSUTLDTC` outcome of a
/// date every `CSUTLDPY` edit passed (P-CSUTLDPY-014), so such an Open Date
/// adds no entry: only 19xx and 20xx years pass the century edit, and
/// 29 February needs a year divisible by 4, and by 400 when YY is 00.
async fn century_and_leap_years(
    app: &TestApp,
    token: &str,
    base: &Map<String, Value>,
    plan: &EditPlan,
) {
    let century =
        messages::date_edit_message(messages::FIELD_OPEN_DATE, messages::CENTURY_NOT_VALID);
    assert_eq!(century, plan.row("V-COACTUPC-015").text);
    let leap = messages::date_edit_message(
        messages::FIELD_OPEN_DATE,
        messages::NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS,
    );
    assert_eq!(leap, plan.row("V-COACTUPC-022").text);
    // The Open Date fields of the year, month and day rows.
    let [year, month, day] = ["V-COACTUPC-013", "V-COACTUPC-016", "V-COACTUPC-018"]
        .map(|id| plan.row(id).row.field.as_str());
    let json = |bms: &str| json_name(bms).unwrap_or_else(|| panic!("{bms} is no body field"));
    let cases: [DateCase; 8] = [
        ("1900", "11", "20", vec![]),
        ("1999", "11", "20", vec![]),
        ("2000", "11", "20", vec![]),
        ("2099", "11", "20", vec![]),
        ("1899", "11", "20", vec![(year, century.as_str())]),
        ("2100", "11", "20", vec![(year, century.as_str())]),
        ("2000", "02", "29", vec![]),
        (
            "1900",
            "02",
            "29",
            vec![
                (year, leap.as_str()),
                (month, leap.as_str()),
                (day, leap.as_str()),
            ],
        ),
    ];
    let status = &plan.status_row;
    for (ccyy, mm, dd, flagged) in cases {
        let date = [(json(year), ccyy), (json(month), mm), (json(day), dd)];
        let mut changes = vec![(plan.status_input.0, plan.status_input.1.as_str())];
        changes.extend(date);
        let resp = app
            .put(ACCOUNT_PATH, Some(token), &changed(base, &changes))
            .await;
        let mut expected = vec![(status.field.as_str(), plan.status_text.as_str())];
        expected.extend(flagged);
        assert_error(
            &resp,
            status.status,
            code(status),
            &plan.status_text,
            &expected,
        );
    }
}

/// The request shape, the key edit, the cross-reference read and the
/// comparison with the fetched values — P-COACTUPC-003, P-COACTUPC-004,
/// P-COACTUPC-005, P-COACTUPC-018, P-COACTUPC-043, P-COACTUPC-048,
/// V-COACTUPC-001, V-COACTUPC-002, V-COACTUPC-003, V-COACTUPC-004,
/// V-COACTUPC-010, E-24
#[test]
fn version_key_and_unchanged_values_are_answered_before_any_edit() {
    let doc = validation_audit();
    let app = TestApp::start();
    block_on(async {
        let token = app.user_token().await;
        let fetched = fetch(&app, &token).await;
        let body = update_body(&fetched);
        version_token(&app, &token, &doc, &body).await;
        key_edit_and_cross_reference(&app, &token, &doc, &body).await;
        unchanged_values(&app, &token, &doc, &fetched, &body).await;
        assert_eq!(
            fetch(&app, &token).await,
            fetched,
            "no answer here may write"
        );
    });
}

/// 9700-CHECK-CHANGE-IN-REC needs the fetched values the version token
/// stands for — P-COACTUPC-048, V-COACTUPC-001, E-24
///
/// An absent or malformed token is the no-message 400 of E-24, before any
/// COBOL edit: a rejected status sent with it gets no status text.
async fn version_token(app: &TestApp, token: &str, doc: &Document, body: &Map<String, Value>) {
    let row = audit_row(doc, MISSING_VERSION_ROW);
    assert!(
        row.template.is_none(),
        "{MISSING_VERSION_ROW} records no message, E-24 naming none"
    );
    let (status_field, status_value) = status_input(&audit_row(doc, STATUS_ROW));
    let mut rejected_status = body.clone();
    set(
        &mut rejected_status,
        &[(status_field, status_value.as_str())],
    );
    let sent = [
        without(body, "version"),
        changed(body, &[("version", MALFORMED_VERSION)]),
        without(&rejected_status, "version"),
    ];
    for sent in sent {
        let resp = app.put(ACCOUNT_PATH, Some(token), &sent).await;
        assert_error(&resp, row.status, code(&row), MISSING_VERSION_MESSAGE, &[]);
    }
}

/// COACTUPC 1210-EDIT-ACCOUNT and 9200-GETCARDXREF-BYACCT — P-COACTUPC-003,
/// P-COACTUPC-018, P-COACTUPC-043, V-COACTUPC-002, V-COACTUPC-003,
/// V-COACTUPC-004
///
/// The `{id}` segment stands for `ACCTSID`: a segment of spaces is not
/// entered, one that is not 11 digits or is all zeros is invalid, and an
/// account with no cross-reference answers the `NOTFND` text E-44 fills in.
async fn key_edit_and_cross_reference(
    app: &TestApp,
    token: &str,
    doc: &Document,
    body: &Map<String, Value>,
) {
    let body = Value::Object(body.clone());
    let blank = audit_row(doc, "V-COACTUPC-002");
    let invalid = audit_row(doc, "V-COACTUPC-003");
    let mut keyed = vec![(
        example_path(&blank).to_owned(),
        &blank,
        catalog_text(&blank),
    )];
    let invalid_text = catalog_text(&invalid);
    let invalid_accounts = quoted_values(&invalid);
    assert!(
        !invalid_accounts.is_empty(),
        "V-COACTUPC-003 names its failing accounts"
    );
    keyed.extend(invalid_accounts.into_iter().map(|(_, account)| {
        (
            format!("/accounts/{account}"),
            &invalid,
            invalid_text.clone(),
        )
    }));
    let unknown = audit_row(doc, "V-COACTUPC-004");
    let account = example_account(&unknown);
    let not_found = messages::xref_not_found_message(account, RESP_NOTFND.0, RESP_NOTFND.1);
    assert_eq!(
        not_found,
        worked_text(&unknown),
        "V-COACTUPC-004 worked text"
    );
    keyed.push((format!("/accounts/{account}"), &unknown, not_found));
    for (path, row, text) in keyed {
        let resp = app.put(&path, Some(token), &body).await;
        assert_error(
            &resp,
            row.status,
            code(row),
            &text,
            &[(row.field.as_str(), text.as_str())],
        );
    }
}

/// COACTUPC 1205-COMPARE-OLD-NEW — P-COACTUPC-005, V-COACTUPC-010
///
/// The fetched values, sent back as fetched or as the comparison reads them
/// (an amount by its `S9(10)V99` value, a name trimmed and upper-cased),
/// change nothing: 200 with the no-change text and the records as fetched,
/// token included.
async fn unchanged_values(
    app: &TestApp,
    token: &str,
    doc: &Document,
    fetched: &Value,
    body: &Map<String, Value>,
) {
    let row = audit_row(doc, "V-COACTUPC-010");
    let text = catalog_text(&row);
    let limit = fetched_text(fetched, "acct_credit_limit")
        .trim_end_matches('0')
        .trim_end_matches('.');
    let first_name = fetched_text(fetched, "cust_first_name").to_uppercase();
    let sent = [
        ("as fetched", Value::Object(body.clone())),
        (
            "as compared",
            changed(
                body,
                &[
                    ("acct_credit_limit", limit),
                    ("cust_first_name", &first_name),
                ],
            ),
        ),
    ];
    for (case, sent) in sent {
        let (status, answer, shown) = app.put(ACCOUNT_PATH, Some(token), &sent).await;
        assert_eq!(status, row.status, "{case}: {shown}");
        assert_eq!(
            answer["message"].as_str(),
            Some(text.as_str()),
            "{case}: {shown}"
        );
        assert_eq!(
            &answer["account"], fetched,
            "{case}: the records as fetched: {shown}"
        );
    }
}

/// COACTUPC 9000-READ-ACCT: the cross-reference, account and customer reads
/// of the fetch — P-COACTUPC-043, P-COACTUPC-044, P-COACTUPC-045,
/// P-COACTUPC-050, V-COACTUPC-005, V-COACTUPC-006, V-COACTUPC-008,
/// V-COACTUPC-009, E-44, E-45
///
/// The fixture holds every record these rows need missing or unreadable, so
/// each is made so by SQL first: a hidden table fails its read with the
/// `IOERR` codes E-44 gives, and a deleted row is a `NOTFND`.
/// V-COACTUPC-007, the failed account read, is the case of
/// `account_read_isolation.rs`.
#[test]
fn failed_and_missing_reads_answer_with_their_file_texts() {
    let doc = validation_audit();
    let app = TestApp::start();
    block_on(async {
        let token = app.user_token().await;
        let fetched = fetch(&app, &token).await;
        let body = Value::Object(update_body(&fetched));
        let reads = [
            ("V-COACTUPC-005", HIDE_XREF, RESTORE_XREF),
            ("V-COACTUPC-009", HIDE_CUSTOMERS, RESTORE_CUSTOMERS),
        ];
        for (id, hide, restore) in reads {
            let row = audit_row(&doc, id);
            app.stack().exec_sql(hide).await;
            let resp = app.put(ACCOUNT_PATH, Some(&token), &body).await;
            app.stack().exec_sql(restore).await;
            assert_file_error(&resp, &row);
        }
        missing_records(&app, &token, &doc, &body).await;
    });
}

/// Asserts that `resp` is `row`'s file-error answer: the `READ` of the file
/// its message cell names, with the `IOERR` codes, at its field.
fn assert_file_error(resp: &Resp, row: &AuditRow) {
    let text = messages::file_error_message(
        ERROR_OPNAME_READ,
        error_file(row),
        RESP_IOERR.0,
        RESP_IOERR.1,
    );
    assert_error(
        resp,
        row.status,
        code(row),
        &text,
        &[(row.field.as_str(), text.as_str())],
    );
}

/// COACTUPC 9300-GETACCTDATA-BYACCT and 9400-GETCUSTDATA-BYCUST `NOTFND` —
/// P-COACTUPC-044, P-COACTUPC-045, P-COACTUPC-050, V-COACTUPC-006,
/// V-COACTUPC-008, V-COACTUPC-009, E-45
///
/// Account 2 without its master record keeps the account text, because the
/// customer read that still runs finds the customer its cross-reference
/// names; a failing customer read replaces that text with the `CUSTDAT` file
/// error. The customer of account 3 without its master record answers the
/// customer text, naming that customer as the `cardxref.txt` record of
/// account 3 holds it, decoded at its reviewed section A offsets
/// (P-CVACT03Y-002, P-CVACT03Y-003). The two accounts' decoded customers
/// differ, so the deleted customer is not the one account 2 still finds.
async fn missing_records(app: &TestApp, token: &str, doc: &Document, body: &Value) {
    let account_row = audit_row(doc, "V-COACTUPC-006");
    let customer_row = audit_row(doc, "V-COACTUPC-008");
    let customer_failure = audit_row(doc, "V-COACTUPC-009");
    let parity = parity_audit();
    let account_3_xref = fixture_records::account_xref(&parity, ACCOUNT_3);
    let missing_customer = account_3_xref.value(XREF_CUSTOMER);
    assert_ne!(
        fixture_records::account_xref(&parity, ACCOUNT_2).value(XREF_CUSTOMER),
        missing_customer,
        "P-CVACT03Y-002, P-CVACT03Y-003: account 2 and account 3 cross-reference different \
         customers"
    );
    app.stack().exec_sql(DELETE_ACCOUNT_2).await;
    app.stack().exec_sql(DELETE_CUSTOMER_OF_ACCOUNT_3).await;
    let account_text =
        messages::account_master_not_found_message(ACCOUNT_2, RESP_NOTFND.0, RESP_NOTFND.1);
    let customer_text =
        messages::customer_master_not_found_message(missing_customer, RESP_NOTFND.0, RESP_NOTFND.1);
    let missing = [
        (ACCOUNT_2, &account_row, account_text),
        (ACCOUNT_3, &customer_row, customer_text),
    ];
    for (account, row, text) in missing {
        let resp = app
            .put(&format!("/accounts/{account}"), Some(token), body)
            .await;
        assert_error(
            &resp,
            row.status,
            code(row),
            &text,
            &[(row.field.as_str(), text.as_str())],
        );
    }
    app.stack().exec_sql(HIDE_CUSTOMERS).await;
    let replaced = app
        .put(&format!("/accounts/{ACCOUNT_2}"), Some(token), body)
        .await;
    app.stack().exec_sql(RESTORE_CUSTOMERS).await;
    assert_file_error(&replaced, &customer_failure);
}

/// A change every edit passes, sent with the current token, with a token
/// another writer has since outdated, and with an amount `NUMVAL-C` does not
/// pin — P-COACTUPC-008, P-COACTUPC-009, P-COACTUPC-012, P-COACTUPC-015,
/// P-COACTUPC-049, V-COACTUPC-023, V-COACTUPC-126, E-26, E-40
///
/// Every one is answered with the unexpected-error outcome of an input class
/// held at `halt` and writes nothing: the Open Date reaches `CSUTLDTC` with
/// no message set (E-26), before the lock, the 9700 change check and the
/// rewrites, and the midpoint amount is held when it is received, before any
/// edit (E-40), so even a rejected status sent with it gets no status text.
#[test]
fn a_change_that_passes_every_edit_is_held_at_the_csutldtc_halt() {
    assert_csutldtc_halt();
    let app = TestApp::start();
    block_on(async {
        let token = app.user_token().await;
        let fetched = fetch(&app, &token).await;
        let base = passing_body(&fetched);
        let valid = changed(&base, &VALID_CHANGE);
        let resp = app.put(ACCOUNT_PATH, Some(&token), &valid).await;
        assert_halt(&resp, "a change every edit passes");
        let status = status_input(&audit_row(&validation_audit(), STATUS_ROW));
        let midpoint = changed(
            &base,
            &[
                (status.0, status.1.as_str()),
                ("acct_credit_limit", MIDPOINT_CREDIT_LIMIT),
            ],
        );
        let resp = app.put(ACCOUNT_PATH, Some(&token), &midpoint).await;
        assert_halt(&resp, "a midpoint amount");
        assert_eq!(
            fetch(&app, &token).await,
            fetched,
            "a halt must write nothing"
        );

        app.stack().exec_sql(ACCOUNT_CHANGED_ELSEWHERE).await;
        app.stack().exec_sql(CUSTOMER_CHANGED_ELSEWHERE).await;
        let resp = app.put(ACCOUNT_PATH, Some(&token), &valid).await;
        assert_halt(&resp, "a token another writer outdated");
        let now = fetch(&app, &token).await;
        assert_ne!(
            now["version"], fetched["version"],
            "the other writer's change shows"
        );
        let stored = app.stack().fetch_text_rows(CHANGED_FIELDS_ROW).await;
        assert_eq!(
            stored,
            [changed_elsewhere_values().map(Some).to_vec()],
            "only the other writer's change is stored"
        );
    });
}

/// The values [`CHANGED_FIELDS_ROW`] reads once [`ACCOUNT_CHANGED_ELSEWHERE`]
/// and [`CUSTOMER_CHANGED_ELSEWHERE`] have run: the seeded credit limit of
/// account 1 that [`SEEDED_ACCOUNT_ROW`] records, plus 1 in exact decimal
/// arithmetic, and [`FIRST_NAME_CHANGED_ELSEWHERE`].
///
/// The sum keeps the two decimals the audit writes the limit with, so it
/// reads as the `NUMERIC(12,2)` text of the stored `acct_credit_limit`.
fn changed_elsewhere_values() -> [String; 2] {
    let seeded =
        fixture_records::section_d_value(&parity_audit(), SEEDED_ACCOUNT_ROW, "acct_credit_limit");
    let seeded: Decimal = seeded.parse().unwrap_or_else(|error| {
        panic!("{SEEDED_ACCOUNT_ROW} records acct_credit_limit={seeded}, no decimal: {error}")
    });
    [
        (seeded + Decimal::ONE).to_string(),
        FIRST_NAME_CHANGED_ELSEWHERE.to_owned(),
    ]
}

/// COACTUPC 9600-WRITE-PROCESSING after a failed customer rewrite —
/// P-COACTUPC-013, P-COACTUPC-014, V-COACTUPC-124, V-COACTUPC-125,
/// V-COACTUPC-127, V-COACTUPC-128, E-26
///
/// A test-only trigger fails every customer update. Under `RECOVERY(NONE)`
/// the account rewrite would stand after the customer rewrite failed
/// (P-COACTUPC-014), but the change is held at the E-26 halt before either
/// lock or rewrite: the answer is the halt's, carrying none of the trigger's
/// text, and neither record changes. Its own test, so the trigger reaches no
/// other test's database.
#[test]
fn a_failing_customer_rewrite_is_not_reached_before_the_csutldtc_halt() {
    assert_csutldtc_halt();
    let app = TestApp::start();
    block_on(async {
        let token = app.user_token().await;
        let fetched = fetch(&app, &token).await;
        app.stack().exec_sql(CREATE_FAILING_FUNCTION).await;
        app.stack().exec_sql(CREATE_FAILING_TRIGGER).await;
        let installed = app.stack().fetch_text_rows(FAILING_TRIGGER).await;
        assert_eq!(installed.len(), 1, "the failing trigger is installed");
        let valid = changed(&passing_body(&fetched), &VALID_CHANGE);
        let resp = app.put(ACCOUNT_PATH, Some(&token), &valid).await;
        assert_halt(&resp, "a change with a failing customer rewrite");
        let stored = app.stack().fetch_text_rows(CHANGED_FIELDS_ROW).await;
        let fetched_values = ["acct_credit_limit", "cust_first_name"]
            .map(|name| Some(fetched_text(&fetched, name).to_owned()));
        assert_eq!(
            stored,
            [fetched_values.to_vec()],
            "neither the account nor the customer is rewritten"
        );
    });
}

// --- The CSUTLDPY rows of the parity audit

/// The Paragraph column of `docs/parity/parity-oracle-audit.md`.
const PARAGRAPH: &str = "Paragraph";

/// The Expected result column of `docs/parity/parity-oracle-audit.md`.
const EXPECTED_RESULT: &str = "Expected result";

/// The dates `COACTUPC` performs `EDIT-DATE-CCYYMMDD THRU
/// EDIT-DATE-CCYYMMDD-EXIT` for, in `1200-EDIT-MAP-INPUTS` order
/// [app/cbl/COACTUPC.cbl:L1478-1541]: the label moved to
/// `WS-EDIT-VARIABLE-NAME` and the year, month and day fields of `COACTUP`.
const CSUTLDPY_DATES: [(&str, [&str; 3]); 4] = [
    (messages::FIELD_OPEN_DATE, ["OPNYEAR", "OPNMON", "OPNDAY"]),
    (messages::FIELD_EXPIRY_DATE, ["EXPYEAR", "EXPMON", "EXPDAY"]),
    (
        messages::FIELD_REISSUE_DATE,
        ["RISYEAR", "RISMON", "RISDAY"],
    ),
    (
        messages::FIELD_DATE_OF_BIRTH,
        ["DOBYEAR", "DOBMON", "DOBDAY"],
    ),
];

/// One `CSUTLDPY` edit row of the parity audit and dates that meet its input
/// condition while passing every other edit.
struct DateEditRow {
    /// The `P-CSUTLDPY-nnn` ID, the key the row is read by.
    id: &'static str,
    /// The paragraph the row's Paragraph cell names.
    paragraph: &'static str,
    /// The `carddemo_domain::messages` text the paragraph strings after the
    /// trimmed label, which must be the row's quoted literal.
    suffix: &'static str,
    /// `Empty` for a part not supplied, `Invalid` for every other rejection.
    kind: ErrorKind,
    /// `WS-EDIT-DATE-FLGS` as the range leaves it: the year, month and day
    /// flags. The parts not valid are the `fields` entries, and the first of
    /// them is the field the message names.
    flags: [FlagState; 3],
    /// The year, month and day entered.
    inputs: &'static [[&'static str; 3]],
}

/// The `CSUTLDPY` rows of `EDIT-YEAR-CCYY`, `EDIT-MONTH`, `EDIT-DAY` and
/// `EDIT-DAY-MONTH-YEAR` [app/cpy/CSUTLDPY.cpy:L25-282] proven here by ID. A
/// part not supplied is sent empty, as spaces and as `*`, the two forms
/// `1100-RECEIVE-MAP` turns into `LOW-VALUES`.
const DATE_EDIT_ROWS: [DateEditRow; 8] = [
    DateEditRow {
        id: "P-CSUTLDPY-002",
        paragraph: "EDIT-YEAR-CCYY",
        suffix: messages::YEAR_MUST_BE_SUPPLIED,
        kind: ErrorKind::Empty,
        flags: [FlagState::Blank, FlagState::Valid, FlagState::Valid],
        inputs: &[["", "04", "15"], ["    ", "04", "15"], ["*", "04", "15"]],
    },
    DateEditRow {
        id: "P-CSUTLDPY-003",
        paragraph: "EDIT-YEAR-CCYY",
        suffix: messages::MUST_BE_4_DIGIT_NUMBER,
        kind: ErrorKind::Invalid,
        flags: [FlagState::NotOk, FlagState::Valid, FlagState::Valid],
        inputs: &[
            ["20A2", "04", "15"],
            ["202", "04", "15"],
            ["YYYY", "04", "15"],
        ],
    },
    DateEditRow {
        id: "P-CSUTLDPY-005",
        paragraph: "EDIT-MONTH",
        suffix: messages::MONTH_MUST_BE_SUPPLIED,
        kind: ErrorKind::Empty,
        flags: [FlagState::Valid, FlagState::Blank, FlagState::Valid],
        inputs: &[
            ["2024", "", "15"],
            ["2024", "  ", "15"],
            ["2024", "*", "15"],
        ],
    },
    DateEditRow {
        id: "P-CSUTLDPY-006",
        paragraph: "EDIT-MONTH",
        suffix: messages::MONTH_MUST_BE_A_NUMBER_BETWEEN_1_AND_12,
        kind: ErrorKind::Invalid,
        flags: [FlagState::Valid, FlagState::NotOk, FlagState::Valid],
        inputs: &[
            ["2024", "13", "15"],
            ["2024", "00", "15"],
            ["2024", "99", "15"],
        ],
    },
    DateEditRow {
        id: "P-CSUTLDPY-007",
        paragraph: "EDIT-DAY",
        suffix: messages::DAY_MUST_BE_SUPPLIED,
        kind: ErrorKind::Empty,
        flags: [FlagState::Valid, FlagState::Valid, FlagState::Blank],
        inputs: &[
            ["2024", "04", ""],
            ["2024", "04", "  "],
            ["2024", "04", "*"],
        ],
    },
    DateEditRow {
        id: "P-CSUTLDPY-008",
        paragraph: "EDIT-DAY",
        suffix: messages::DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31,
        kind: ErrorKind::Invalid,
        flags: [FlagState::Valid, FlagState::Valid, FlagState::NotOk],
        inputs: &[
            ["2024", "04", "AB"],
            ["2024", "04", "32"],
            ["2024", "04", "00"],
        ],
    },
    DateEditRow {
        id: "P-CSUTLDPY-009",
        paragraph: "EDIT-DAY-MONTH-YEAR",
        suffix: messages::CANNOT_HAVE_31_DAYS_IN_THIS_MONTH,
        kind: ErrorKind::Invalid,
        flags: [FlagState::Valid, FlagState::NotOk, FlagState::NotOk],
        inputs: &[
            ["2024", "02", "31"],
            ["2024", "04", "31"],
            ["2024", "06", "31"],
            ["2024", "09", "31"],
            ["2024", "11", "31"],
        ],
    },
    DateEditRow {
        id: "P-CSUTLDPY-010",
        paragraph: "EDIT-DAY-MONTH-YEAR",
        suffix: messages::CANNOT_HAVE_30_DAYS_IN_THIS_MONTH,
        kind: ErrorKind::Invalid,
        flags: [FlagState::Valid, FlagState::NotOk, FlagState::NotOk],
        inputs: &[
            ["2024", "02", "30"],
            ["2023", "02", "30"],
            ["2000", "02", "30"],
        ],
    },
];

/// The `CSUTLDPY` row of `EDIT-DATE-LE`, the `CSUTLDTC` call.
const EDIT_DATE_LE_ROW: &str = "P-CSUTLDPY-012";

/// Dates that pass every `CSUTLDPY` edit: 29 February of a year divisible by
/// 4 and of one divisible by 400, the 31st of a 31-day month, the 30th of a
/// 30-day month, and a day `NUMVAL` converts (`5 ` is the fifth).
const CSUTLDPY_VALID_DATES: [[&str; 3]; 5] = [
    ["2024", "02", "29"],
    ["2000", "02", "29"],
    ["1999", "12", "31"],
    ["2024", "04", "30"],
    ["2024", "04", "5 "],
];

/// A `CSUTLDTC` severity and message number, those of `FC-BAD-DATE-VALUE`,
/// for the text a non-zero severity gives.
const REJECTED_SEVERITY: (&str, &str) = ("0003", "2508");

/// One input of a [`DateEditRow`] entered as one of [`CSUTLDPY_DATES`].
struct SentDate<'a> {
    /// The row the input meets.
    row: &'a DateEditRow,
    /// The date's label.
    label: &'static str,
    /// The date's year, month and day fields.
    bms: [&'static str; 3],
    /// The year, month and day entered.
    input: [&'static str; 3],
    /// The trimmed label followed by the row's literal.
    text: String,
}

impl SentDate<'_> {
    /// The `(field, text)` entries of the parts the row's flags leave not
    /// valid, in year, month, day order.
    fn flagged(&self) -> Vec<(&'static str, &str)> {
        self.bms
            .into_iter()
            .zip(self.row.flags)
            .filter(|(_, flag)| *flag != FlagState::Valid)
            .map(|(field, _)| (field, self.text.as_str()))
            .collect()
    }

    /// The case as an assertion names it.
    fn case(&self) -> String {
        format!("{} {} {:?}", self.row.id, self.label, self.input)
    }
}

/// The trimmed cell `column` of the parity audit row `id`.
fn parity_cell<'a>(doc: &'a Document, id: &str, column: &str) -> &'a str {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    table
        .cell(row, column)
        .unwrap_or_else(|| panic!("{id} has no `{column}` cell"))
        .trim()
}

/// A code span's text without the double quotes around it.
fn unquote<'a>(text: &'a str, id: &str) -> &'a str {
    text.strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id}: {text} is not a double-quoted string"))
}

/// `FUNCTION TRIM(WS-EDIT-VARIABLE-NAME)` after `MOVE label TO
/// WS-EDIT-VARIABLE-NAME`: the label cut to the `X(25)` receiver, trimmed.
fn edit_name(label: &str) -> &str {
    label.get(..EDIT_NAME_LEN).unwrap_or(label).trim()
}

/// The year, month and day fields of a date as `DateBmsFields`.
fn bms_fields([year, month, day]: [&'static str; 3]) -> DateBmsFields {
    DateBmsFields { year, month, day }
}

/// The date parts `1100-RECEIVE-MAP` moves for `entered`: a part of spaces or
/// a lone `*` becomes `LOW-VALUES` (P-COACTUPC-019), any other part is moved
/// into its `PIC X` item.
fn received_parts([year, month, day]: [&str; 3]) -> DateParts {
    let blank = |text: &str| matches!(text.trim_end(), "" | "*");
    let mut parts = DateParts::from_entered(year, month, day);
    if blank(year) {
        parts.ccyy = [0; 4];
    }
    if blank(month) {
        parts.mm = [0; 2];
    }
    if blank(day) {
        parts.dd = [0; 2];
    }
    parts
}

/// The changes that enter `input` as the date of `bms`.
fn date_changes(bms: [&str; 3], input: [&'static str; 3]) -> Vec<(&'static str, &'static str)> {
    bms.into_iter()
        .zip(input)
        .map(|(field, value)| {
            let json = json_name(field).unwrap_or_else(|| panic!("{field} is no body field"));
            (json, value)
        })
        .collect()
}

/// The literal `row`'s paragraph strings after the label, read from its
/// parity audit row, which must name `row.paragraph`, quote the catalog text
/// and, where it records a `flag=`, give the flag of the part it rejects.
fn audited_literal(parity: &Document, row: &DateEditRow) -> String {
    let id = row.id;
    let paragraph = parity_cell(parity, id, PARAGRAPH);
    assert_eq!(
        paragraph.split_whitespace().next(),
        Some(row.paragraph),
        "{id} is a row of {paragraph}"
    );
    let expected = parity_cell(parity, id, EXPECTED_RESULT);
    let literal = quoted_text(expected, id);
    assert_eq!(
        literal, row.suffix,
        "{id}: the catalog text differs from the audited literal"
    );
    if let Some(flag) = key_values(expected)
        .into_iter()
        .find(|pair| pair.key == "flag")
    {
        let state = match flag.value.as_str() {
            "B" => FlagState::Blank,
            "0" => FlagState::NotOk,
            other => panic!("{id} records an unknown flag {other:?}"),
        };
        let rejected = row.flags.into_iter().find(|part| *part != FlagState::Valid);
        assert_eq!(rejected, Some(state), "{id}: flag={}", flag.value);
    }
    literal
}

/// CSUTLDPY EDIT-YEAR-CCYY, EDIT-MONTH, EDIT-DAY and EDIT-DAY-MONTH-YEAR —
/// P-CSUTLDPY-002, P-CSUTLDPY-003, P-CSUTLDPY-005, P-CSUTLDPY-006,
/// P-CSUTLDPY-007, P-CSUTLDPY-008, P-CSUTLDPY-009, P-CSUTLDPY-010
///
/// Each row of [`DATE_EDIT_ROWS`] is read by its ID: its paragraph, its
/// literal, which must be the catalog text, and its `flag=` where it records
/// one. Each input is then edited as each of the four dates, the text being
/// the trimmed label followed by that literal: by `edit_date_ccyymmdd`, whose
/// flags and first message are the row's, and by `PUT`. A failing date never
/// reaches `CSUTLDTC`, so the edit answers its flags and no request is held
/// at the E-26 halt.
#[test]
fn every_csutldpy_edit_row_rejects_each_date_with_its_text() {
    let parity = parity_audit();
    let plan = edit_plan(&validation_audit());
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        let fetched = fetch(&app, &token).await;
        let base = passing_body(&fetched);
        for row in &DATE_EDIT_ROWS {
            let literal = audited_literal(&parity, row);
            for (label, bms) in CSUTLDPY_DATES {
                for &input in row.inputs {
                    let sent = SentDate {
                        row,
                        label,
                        bms,
                        input,
                        text: format!("{}{literal}", edit_name(label)),
                    };
                    assert_rule_edit(&sent);
                    send_date_edit(&app, &token, &base, &plan, &sent).await;
                }
            }
        }
        assert_eq!(
            fetch(&app, &token).await,
            fetched,
            "a rejected date must write nothing"
        );
    });
}

/// Asserts that `edit_date_ccyymmdd` leaves the flags of `sent`'s row and
/// writes its text on the first part those flags leave not valid.
fn assert_rule_edit(sent: &SentDate<'_>) {
    let case = sent.case();
    let mut msg = ReturnMsg::default();
    let parts = received_parts(sent.input);
    let result = edit_date_ccyymmdd(sent.label, &parts, &mut msg, bms_fields(sent.bms));
    let [year, month, day] = sent.row.flags;
    assert_eq!(
        result,
        Ok(DateEditFlags { year, month, day }),
        "{case}: the flags"
    );
    assert_eq!(
        msg.message(),
        Some(sent.text.as_str()),
        "{case}: the message"
    );
    assert_eq!(
        msg.field(),
        sent.flagged().first().map(|(field, _)| *field),
        "{case}: the field the message names"
    );
}

/// Sends `sent` on `base` and asserts its answer. The Open Date, the first
/// date edited, answers its own text and kind, with the 400 of both kinds,
/// and its entries. A later date follows an Open Date that passes every edit
/// and so reaches `CSUTLDTC`, so it is sent behind the rejected status: the
/// status's answer with the date's entries after the status entry.
async fn send_date_edit(
    app: &TestApp,
    token: &str,
    base: &Map<String, Value>,
    plan: &EditPlan,
    sent: &SentDate<'_>,
) {
    let own_message = sent.label == messages::FIELD_OPEN_DATE;
    let status = (plan.status_input.0, plan.status_input.1.as_str());
    let changes: Vec<(&str, &str)> = (!own_message)
        .then_some(status)
        .into_iter()
        .chain(date_changes(sent.bms, sent.input))
        .collect();
    let resp = app
        .put(ACCOUNT_PATH, Some(token), &changed(base, &changes))
        .await;
    let flagged = sent.flagged();
    assert!(!flagged.is_empty(), "{}: no part is flagged", sent.case());
    if own_message {
        let kind_code = sent.row.kind.code();
        assert_error(
            &resp,
            StatusCode::BAD_REQUEST,
            kind_code,
            &sent.text,
            &flagged,
        );
    } else {
        let status_row = &plan.status_row;
        let mut expected = vec![(status_row.field.as_str(), plan.status_text.as_str())];
        expected.extend(flagged);
        assert_error(
            &resp,
            status_row.status,
            code(status_row),
            &plan.status_text,
            &expected,
        );
    }
}

/// CSUTLDPY EDIT-DATE-LE — P-CSUTLDPY-012, E-26
///
/// The row calls `CSUTLDTC` with the picture `YYYYMMDD` once every `CSUTLDPY`
/// edit has passed, and gives such dates the E-26 halt. `edit_date_ccyymmdd`
/// answers each of the four dates of [`CSUTLDPY_VALID_DATES`] with the E-26
/// `Unproven` and no message, which only the `YYYYMMDD` call gives, while the
/// dates of [`DATE_EDIT_ROWS`], which fail an edit, answer their flags. `PUT`
/// answers an Open Date that passes every edit with the halt's 500 and writes
/// nothing. The text of a non-zero severity, which the halt leaves unreached,
/// is the trimmed label with the row's two literals, the severity and the
/// message number.
#[test]
fn a_date_that_passes_every_csutldpy_edit_reaches_the_csutldtc_halt() {
    assert_csutldtc_halt();
    assert_edit_date_le_row(&parity_audit());
    let unproven = Err(DateValidationError::Unproven {
        escalation_id: CSUTLDTC_ESCALATION,
    });
    for (label, bms) in CSUTLDPY_DATES {
        for date in CSUTLDPY_VALID_DATES {
            let mut msg = ReturnMsg::default();
            let parts = received_parts(date);
            let result = edit_date_ccyymmdd(label, &parts, &mut msg, bms_fields(bms));
            assert_eq!(result, unproven, "{label} {date:?}");
            assert!(msg.is_off(), "{label} {date:?}: the halt sets no message");
        }
    }
    let [(_, open_date), ..] = CSUTLDPY_DATES;
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        let fetched = fetch(&app, &token).await;
        let base = passing_body(&fetched);
        for date in CSUTLDPY_VALID_DATES {
            let changes = date_changes(open_date, date);
            let resp = app
                .put(ACCOUNT_PATH, Some(&token), &changed(&base, &changes))
                .await;
            assert_halt(&resp, &format!("an Open Date {date:?} every edit passes"));
        }
        assert_eq!(
            fetch(&app, &token).await,
            fetched,
            "a halt must write nothing"
        );
    });
}

/// Asserts the `EDIT-DATE-LE` row: its call's picture is the one the port
/// passes, a date every edit passed is the E-26 halt, and the text of a
/// non-zero severity strings, for each of the four dates, the trimmed label,
/// the row's first literal, the severity, its second literal and the message
/// number.
fn assert_edit_date_le_row(parity: &Document) {
    let id = EDIT_DATE_LE_ROW;
    let expected = parity_cell(parity, id, EXPECTED_RESULT);
    let call = key_values(expected)
        .into_iter()
        .find(|pair| pair.key == "call")
        .map_or_else(|| panic!("{id} records no call="), |pair| pair.value);
    let picture = call
        .split('\'')
        .nth(1)
        .unwrap_or_else(|| panic!("{id}: the call quotes no picture: {call}"));
    assert!(
        DatePicture::YyyyMmDd
            .as_bytes()
            .starts_with(picture.as_bytes()),
        "{id}: CSUTLDPY passes the picture {picture}"
    );
    assert!(
        expected.contains(&format!("{HALT} {CSUTLDTC_ESCALATION}")),
        "{id}: a date every edit passed is {HALT} {CSUTLDTC_ESCALATION}: {expected}"
    );
    let spans = cell_code_spans(expected);
    let literals: Vec<&str> = spans.iter().map(|span| unquote(&span.text, id)).collect();
    assert_eq!(
        literals,
        [messages::VALIDATION_ERROR_SEV_CODE, messages::MESSAGE_CODE],
        "{id}: the catalog texts differ from the audited literals"
    );
    let (severity, number) = REJECTED_SEVERITY;
    for (label, _) in CSUTLDPY_DATES {
        let audited = format!(
            "{}{}{severity}{}{number}",
            edit_name(label),
            literals[0],
            literals[1]
        );
        assert_eq!(
            messages::date_validation_error_message(label, severity, number),
            audited,
            "{id}: {label}"
        );
    }
}

// --- The CSLKPCDY area-code rows of the parity audit

/// `CSLKPCDY` `88 VALID-PHONE-AREA-CODE`: every North American area code.
const PHONE_AREA_CODE_ROW: &str = "P-CSLKPCDY-001";

/// `CSLKPCDY` `88 VALID-GENERAL-PURP-CODE`: the codes `EDIT-AREA-CODE` tests.
const GENERAL_PURP_CODE_ROW: &str = "P-CSLKPCDY-002";

/// `CSLKPCDY` `88 VALID-EASY-RECOG-AREA-CODE`: the easily recognisable codes.
const EASY_RECOG_AREA_CODE_ROW: &str = "P-CSLKPCDY-003";

/// The rows of an area code outside `VALID-GENERAL-PURP-CODE`, for Phone
/// Number 1 and Phone Number 2.
const AREA_CODE_ROWS: [&str; 2] = ["V-COACTUPC-101", "V-COACTUPC-111"];

/// The codes of a `CSLKPCDY` table row: its `values=` list, which holds the
/// `count=` codes the row records.
fn audited_codes(parity: &Document, id: &str) -> Vec<String> {
    let pairs = key_values(parity_cell(parity, id, EXPECTED_RESULT));
    let value = |key: &str| {
        pairs.iter().find(|pair| pair.key == key).map_or_else(
            || panic!("{id} records no `{key}=`"),
            |pair| pair.value.clone(),
        )
    };
    let codes: Vec<String> = value("values")
        .split(',')
        .map(|area| area.trim().to_owned())
        .collect();
    let count: usize = value("count")
        .parse()
        .unwrap_or_else(|_| panic!("{id}: `count=` is no number"));
    assert_eq!(codes.len(), count, "{id}: `values=` holds `count=` codes");
    codes
}

/// CSLKPCDY VALID-PHONE-AREA-CODE and VALID-EASY-RECOG-AREA-CODE, as COACTUPC
/// EDIT-AREA-CODE uses them — P-CSLKPCDY-001, P-CSLKPCDY-003,
/// P-COACTUPC-028, V-COACTUPC-101, V-COACTUPC-111
///
/// Each ported table holds its row's codes in copybook order, and the North
/// American codes are the general-purpose codes followed by the easily
/// recognisable ones. `EDIT-AREA-CODE` tests only the general-purpose codes,
/// so every North American code is sent as the area code of both phones,
/// behind the rejected status: a general-purpose code adds no entry, and an
/// easily recognisable one adds each phone's not-valid entry.
#[test]
fn of_the_north_american_area_codes_only_general_purpose_codes_pass() {
    let parity = parity_audit();
    let phone = audited_codes(&parity, PHONE_AREA_CODE_ROW);
    let general = audited_codes(&parity, GENERAL_PURP_CODE_ROW);
    let easy = audited_codes(&parity, EASY_RECOG_AREA_CODE_ROW);
    assert_eq!(
        phone, VALID_PHONE_AREA_CODE,
        "{PHONE_AREA_CODE_ROW}: the ported table"
    );
    assert_eq!(
        easy, VALID_EASY_RECOG_AREA_CODE,
        "{EASY_RECOG_AREA_CODE_ROW}: the ported table"
    );
    assert_eq!(
        phone,
        [general.as_slice(), easy.as_slice()].concat(),
        "{PHONE_AREA_CODE_ROW} is {GENERAL_PURP_CODE_ROW} followed by {EASY_RECOG_AREA_CODE_ROW}"
    );
    let plan = edit_plan(&validation_audit());
    let app = TestApp::start();
    block_on(async {
        let token = app.user_token().await;
        let fetched = fetch(&app, &token).await;
        let base = passing_body(&fetched);
        for area in &phone {
            let general_purpose = general.contains(area);
            send_area_code(&app, &token, &base, &plan, area, general_purpose).await;
        }
        assert_eq!(
            fetch(&app, &token).await,
            fetched,
            "a rejected change must write nothing"
        );
    });
}

/// Sends `area` as the area code of both phones on `base`, behind the
/// rejected status, and asserts the status's answer: the status entry, then,
/// unless `general_purpose`, the not-valid entry of each phone.
async fn send_area_code(
    app: &TestApp,
    token: &str,
    base: &Map<String, Value>,
    plan: &EditPlan,
    area: &str,
    general_purpose: bool,
) {
    let case = format!("area code {area}");
    let phones = AREA_CODE_ROWS.map(|id| plan.row(id));
    let mut changes = vec![(plan.status_input.0, plan.status_input.1.as_str())];
    for edit in &phones {
        let field = edit.row.field.as_str();
        let json = json_name(field).unwrap_or_else(|| panic!("{field} is no body field"));
        changes.push((json, area));
    }
    let resp = app
        .put(ACCOUNT_PATH, Some(token), &changed(base, &changes))
        .await;
    let status_row = &plan.status_row;
    let mut expected = vec![(status_row.field.clone(), plan.status_text.clone())];
    if !general_purpose {
        expected.extend(
            phones
                .iter()
                .map(|edit| (edit.row.field.clone(), edit.text.clone())),
        );
    }
    let (status, body, raw) = &resp;
    assert_eq!(*status, status_row.status, "{case}: status: {raw}");
    assert_eq!(
        body["code"].as_str(),
        Some(code(status_row)),
        "{case}: code: {raw}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(plan.status_text.as_str()),
        "{case}: the status's text stays the message: {raw}"
    );
    assert_eq!(
        entries(&resp),
        expected,
        "{case}: the flagged fields: {raw}"
    );
}
