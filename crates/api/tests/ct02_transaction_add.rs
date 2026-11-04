//! CT02 `POST /transactions` (`COTRN02C`, map `COTRN02`) through the HTTP
//! layer: the audited edits in the COBOL order, the audited I/O failures, the
//! TRAN-ID allocation, and the reward accrual that commits, or rolls back, with
//! the insert (AAP 0.3.1, 0.3.5, 0.5.3.6, 0.10.1).
//!
//! Every rejection is checked against its `V-COTRN02C` row of
//! `docs/parity/validation-audit.md`: the status, the error code of the row's
//! kind, the BMS field and the (Order, ID) rank come from the row, and the
//! expected text is the `carddemo_domain::messages` constant, asserted equal
//! to the row's quoted text. The success text and status are those of
//! P-COTRN02C-024 in `docs/parity/parity-oracle-audit.md`.
//!
//! The seeded live master is empty (E-07), and a confirmed add against an
//! empty live master is not found and adds nothing (V-COTRN02C-029, E-27):
//! the `READPREV` `ENDFILE` path that would allocate `0000000000000001` is
//! unreachable (P-COTRN02C-022). Each suite therefore puts live rows in place
//! with SQL before an add is expected to succeed.
//!
//! Dates of the `CEEDAYS` classes the audit holds at halt (year `0000`, and a
//! date outside 1582-10-15 to 9999-12-31 that is not a calendar date: E-31)
//! are never sent: only the accepted and rejected classes the reviewed rows
//! pin are asserted. The `CSUTLDTC` call itself, the 80-byte result CT02 reads
//! and its feedback classes are checked on the port CT02 calls,
//! `carddemo_domain::rules::date_validation`, which answers a halt class with
//! its escalation rather than a feedback.
//!
//! The 3270 mechanics of the map (the AID keys, the transfers, PF4 Clear, the
//! header) are checked as their REST replacements: the operations and
//! members the API document publishes, the answer without a session, and the
//! requests that stand for PF5 and for a key the add has no member for.
//!
//! The I/O failure rows have no fixture input, so they are proven by fault
//! injection: a hidden cross-reference table or live-master view, and
//! test-only insert triggers. Docker must be running; each Docker-backed test
//! function starts its own seeded `postgres:16.15` container, and the harness
//! conventions are those of `common`.

pub mod common;

use std::collections::BTreeSet;
use std::str::FromStr;

use carddemo_audit::{Document, cell_code_spans, code_span, key_values};
use carddemo_domain::messages;
use carddemo_domain::rules::date_validation::{
    self, CeedaysFeedback, CsutldtcResult, DatePicture, DateValidationError,
};
use carddemo_testkit::block_on;
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};

use common::{TestApp, assert_error, fixture_records};

/// The program whose validation rows this file proves.
const PROGRAM: &str = "COTRN02C";

/// The CT02 route; CT01 reads an added record back at `/transactions/{id}`.
const ROUTE: &str = "/transactions";

/// The processing clock, inside the active period of the seeded retail rule
/// (`01`/`0001`, rate 1.0000, from 2022-01-01; AAP 0.3.6). It is the
/// `created_at` of every accrual, and no transaction field reads it
/// (P-COTRN02C-030).
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// Account 1 of the seed (P-CVACT01Y-015).
const ACCOUNT_ID: &str = "00000000001";

/// The `account_id` of account 1 in the reward tables, a `BIGINT` read as
/// text.
const ACCOUNT_KEY: &str = "1";

/// The card number the card-keyed requests enter as `CARDNINI`
/// [app/data/ASCII/cardxref.txt:L49]. The record an add of it writes takes its
/// card from the decoded cross-reference ([`xref_card`]), which must give
/// account [`ACCOUNT_ID`]. It exceeds 2^53, so its exact echo also proves the
/// integer reading of `FUNCTION NUMVAL` (P-COTRN02C-027).
const CARD_NUMBER: &str = "9680294154603697";

/// One member change from [`BASELINE`]: `Some` sets the member, `None` leaves
/// it out of the body, which is a blank field.
type Change = (&'static str, Option<&'static str>);

/// A body every edit passes: account 1, type `01` and category `0001`
/// (P-TRNTYPE-004, P-TRNTYCAT-005), the amount in the form `-99999999.99`,
/// two calendar dates and a nine-digit merchant ID, confirmed with `Y`. Its
/// 12.34 earns points under the seeded retail rule.
const BASELINE: [(&str, &str); 13] = [
    ("account_id", ACCOUNT_ID),
    ("tran_type_cd", "01"),
    ("tran_cat_cd", "0001"),
    ("tran_source", "POS TERM"),
    ("tran_desc", "CT02 test purchase"),
    ("tran_amt", "+00000012.34"),
    ("tran_orig_date", "2022-07-01"),
    ("tran_proc_date", "2022-07-01"),
    ("tran_merchant_id", "800000001"),
    ("tran_merchant_name", "CT02 Test Merchant"),
    ("tran_merchant_city", "Test City"),
    ("tran_merchant_zip", "12345"),
    ("confirm", "Y"),
];

/// The forms of a blank map field: an absent member, an empty one, `SPACES`
/// and `LOW-VALUES`, each within every field's width.
const BLANKS: [Option<&str>; 4] = [None, Some(""), Some(" "), Some("\0")];

/// The highest TRAN-ID of [`LIVE_ROWS`]. A successful add is given the next
/// one, `0000000000001000`: the carry and the zero padding of the `9(16)`
/// working field (P-COTRN02C-020).
const HIGHEST_LIVE: &str = "0000000000000999";

/// Two live rows, the highest key first, and a higher-keyed row that
/// `batch tranbkp` has taken out of the live master, which the allocation
/// must not count (E-09).
const LIVE_ROWS: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) VALUES \
     ('0000000000000999', '01', 1, 'POS TERM', 'Highest live row', 10.00, \
      1, 'Base merchant', 'Base city', '12345', '9680294154603697', '2022-06-01', '2022-06-01'), \
     ('0000000000000123', '01', 1, 'POS TERM', 'Lower live row', 20.00, \
      1, 'Base merchant', 'Base city', '12345', '9680294154603697', '2022-06-02', '2022-06-02'); \
     INSERT INTO transactions \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts, in_master) VALUES \
     ('0000000000005000', '01', 1, 'POS TERM', 'Backed-up row', 30.00, \
      1, 'Base merchant', 'Base city', '12345', '9680294154603697', '2022-05-01', '2022-05-01', \
      FALSE)";

/// The live master's row count.
const LIVE_COUNT: &str = "SELECT count(*)::text FROM transaction_master";

/// The row counts a rejected or rolled-back add leaves unchanged: every
/// transaction row, live or retained, the live rows, and the accruals.
const WRITE_COUNTS: &str = "SELECT (SELECT count(*) FROM transactions)::text, \
     (SELECT count(*) FROM transaction_master)::text, \
     (SELECT count(*) FROM reward_accruals)::text";

/// Every reward balance, by account.
const REWARD_BALANCES: &str = "SELECT account_id::text, total_points_earned::text, \
     total_points_redeemed::text, available_points::text \
     FROM reward_balances ORDER BY account_id";

/// The balances of account 1. CT02 never writes them: its only legacy write
/// is the transaction record (P-COTRN02C-024).
const ACCOUNT_BALANCES: &str = "SELECT acct_curr_bal::text, acct_curr_cyc_credit::text, \
     acct_curr_cyc_debit::text FROM accounts WHERE account_id = 1";

/// The seeded rule of type `01` and category `0001`: its ID and rate.
const RETAIL_RULE: &str = "SELECT rule_id::text, reward_rate::text FROM reward_rules \
     WHERE transaction_type_cd = '01' AND transaction_category_cd = 1";

/// Every accrual in insertion order: the live TRAN-ID it belongs to, its
/// rule, account and points, and its instant in the form of [`CLOCK`].
const ACCRUALS: &str = r#"SELECT t.tran_id, a.rule_id::text, a.account_id::text,
       a.points_earned::text,
       to_char(a.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')
  FROM reward_accruals a JOIN transaction_master t ON t.transaction_id = a.transaction_id
 ORDER BY a.accrual_id"#;

/// The confirmed add of [`BASELINE`] with dates other than the clock's, so the
/// record shows that the timestamps are the entered dates (P-COTRN02C-023).
const QUALIFYING: &[Change] = &[
    ("tran_orig_date", Some("2022-06-14")),
    ("tran_proc_date", Some("2022-06-15")),
];

/// Adds that accrue nothing (AAP 0.3.5): a type no rule names, keyed by card
/// and confirmed with `y` (P-COTRN02C-006, P-COTRN02C-017), at the largest
/// amount the format allows (P-COTRN02C-028); a negative and a zero amount
/// with the retail rule's codes; and a category of type `01` no rule names.
const NON_QUALIFYING: [&[Change]; 4] = [
    &[
        ("account_id", None),
        ("card_number", Some(CARD_NUMBER)),
        ("tran_type_cd", Some("02")),
        ("tran_amt", Some("+99999999.99")),
        ("confirm", Some("y")),
    ],
    &[("tran_amt", Some("-00000005.00"))],
    &[("tran_amt", Some("+00000000.00"))],
    &[("tran_cat_cd", Some("0002"))],
];

/// The blank-field `EVALUATE` in screen order (P-COTRN02C-010): the row, the
/// member and the message of each data field.
const BLANK_ROWS: [(&str, &str, &str); 11] = [
    (
        "V-COTRN02C-008",
        "tran_type_cd",
        messages::TYPE_CD_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-009",
        "tran_cat_cd",
        messages::CATEGORY_CD_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-010",
        "tran_source",
        messages::SOURCE_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-011",
        "tran_desc",
        messages::DESCRIPTION_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-012",
        "tran_amt",
        messages::AMOUNT_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-013",
        "tran_orig_date",
        messages::ORIG_DATE_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-014",
        "tran_proc_date",
        messages::PROC_DATE_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-015",
        "tran_merchant_id",
        messages::MERCHANT_ID_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-016",
        "tran_merchant_name",
        messages::MERCHANT_NAME_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-017",
        "tran_merchant_city",
        messages::MERCHANT_CITY_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-COTRN02C-018",
        "tran_merchant_zip",
        messages::MERCHANT_ZIP_CAN_NOT_BE_EMPTY,
    ),
];

/// The class, format and date edits after the blank-field `EVALUATE`, in the
/// COBOL order (P-COTRN02C-011 to P-COTRN02C-016), then the confirmation
/// values that are neither `Y`, `N` nor blank (P-COTRN02C-019): the row, the
/// member, a failing entry and the message.
///
/// The date rows send calendar-invalid dates inside 1582-10-15 to 9999-12-31
/// only: `CEEDAYS` answers 2508 for a day of 00 or beyond the month
/// (P-CSUTLDTC-003) and 2517 for a month of 00 or 13 (P-CSUTLDTC-005), and
/// COTRN02C rejects both (P-COTRN02C-015).
const EDIT_CASES: [(&str, &str, &str, &str); 21] = [
    (
        "V-COTRN02C-019",
        "tran_type_cd",
        "0A",
        messages::TYPE_CD_MUST_BE_NUMERIC,
    ),
    (
        "V-COTRN02C-019",
        "tran_type_cd",
        "1",
        messages::TYPE_CD_MUST_BE_NUMERIC,
    ),
    (
        "V-COTRN02C-020",
        "tran_cat_cd",
        "00A1",
        messages::CATEGORY_CD_MUST_BE_NUMERIC,
    ),
    (
        "V-COTRN02C-020",
        "tran_cat_cd",
        "001",
        messages::CATEGORY_CD_MUST_BE_NUMERIC,
    ),
    (
        "V-COTRN02C-021",
        "tran_amt",
        "12.50",
        messages::AMOUNT_SHOULD_BE_IN_FORMAT,
    ),
    (
        "V-COTRN02C-021",
        "tran_amt",
        "+00000012.5X",
        messages::AMOUNT_SHOULD_BE_IN_FORMAT,
    ),
    (
        "V-COTRN02C-021",
        "tran_amt",
        "+00000012,34",
        messages::AMOUNT_SHOULD_BE_IN_FORMAT,
    ),
    (
        "V-COTRN02C-021",
        "tran_amt",
        "*00000012.34",
        messages::AMOUNT_SHOULD_BE_IN_FORMAT,
    ),
    (
        "V-COTRN02C-022",
        "tran_orig_date",
        "2022/07/01",
        messages::ORIG_DATE_SHOULD_BE_IN_FORMAT,
    ),
    (
        "V-COTRN02C-022",
        "tran_orig_date",
        "2022-7-1",
        messages::ORIG_DATE_SHOULD_BE_IN_FORMAT,
    ),
    (
        "V-COTRN02C-023",
        "tran_proc_date",
        "20220701",
        messages::PROC_DATE_SHOULD_BE_IN_FORMAT,
    ),
    (
        "V-COTRN02C-023",
        "tran_proc_date",
        "2022-07-1",
        messages::PROC_DATE_SHOULD_BE_IN_FORMAT,
    ),
    (
        "V-COTRN02C-024",
        "tran_orig_date",
        "2022-02-30",
        messages::ORIG_DATE_NOT_A_VALID_DATE,
    ),
    (
        "V-COTRN02C-024",
        "tran_orig_date",
        "2022-07-00",
        messages::ORIG_DATE_NOT_A_VALID_DATE,
    ),
    (
        "V-COTRN02C-024",
        "tran_orig_date",
        "2022-00-15",
        messages::ORIG_DATE_NOT_A_VALID_DATE,
    ),
    (
        "V-COTRN02C-025",
        "tran_proc_date",
        "2022-13-01",
        messages::PROC_DATE_NOT_A_VALID_DATE,
    ),
    (
        "V-COTRN02C-025",
        "tran_proc_date",
        "2023-02-29",
        messages::PROC_DATE_NOT_A_VALID_DATE,
    ),
    (
        "V-COTRN02C-026",
        "tran_merchant_id",
        "12345678A",
        messages::MERCHANT_ID_MUST_BE_NUMERIC,
    ),
    (
        "V-COTRN02C-026",
        "tran_merchant_id",
        "1234",
        messages::MERCHANT_ID_MUST_BE_NUMERIC,
    ),
    (
        "V-COTRN02C-028",
        "confirm",
        "X",
        messages::INVALID_VALUE_VALID_VALUES_ARE_Y_N,
    ),
    (
        "V-COTRN02C-028",
        "confirm",
        "1",
        messages::INVALID_VALUE_VALID_VALUES_ARE_Y_N,
    ),
];

/// Dates `CSUTLDTC` accepts, which therefore reach the confirmation: severity
/// `0000` for calendar dates inside 1582-10-15 to 9999-12-31, its bounds and a
/// leap day included (P-CSUTLDTC-002), and message 2513 for calendar dates
/// before that range with a year of 0001 or later (P-CSUTLDTC-004), which
/// COTRN02C accepts as valid (P-COTRN02C-015).
const ACCEPTED_DATES: [&str; 5] = [
    "1582-10-15",
    "9999-12-31",
    "2024-02-29",
    "1582-10-14",
    "0001-01-01",
];

/// Inputs that fail two rows, as (first row, later row, message of the first,
/// changes): `SEND-TRNADD-SCREEN` ends the task at the first failure
/// (P-COTRN02C-003), in the order key fields, data fields, confirmation
/// (P-COTRN02C-004), so the row first in the audit's (Order, ID) order
/// answers.
const PRECEDENCE: [(&str, &str, &str, &[Change]); 8] = [
    (
        "V-COTRN02C-001",
        "V-COTRN02C-008",
        messages::ACCOUNT_ID_MUST_BE_NUMERIC,
        &[("account_id", Some("0000000000A")), ("tran_type_cd", None)],
    ),
    (
        "V-COTRN02C-004",
        "V-COTRN02C-008",
        messages::ACCOUNT_ID_NOT_FOUND,
        &[("account_id", Some("99999999999")), ("tran_type_cd", None)],
    ),
    (
        "V-COTRN02C-008",
        "V-COTRN02C-012",
        messages::TYPE_CD_CAN_NOT_BE_EMPTY,
        &[("tran_amt", Some("")), ("tran_type_cd", Some(""))],
    ),
    (
        "V-COTRN02C-009",
        "V-COTRN02C-019",
        messages::CATEGORY_CD_CAN_NOT_BE_EMPTY,
        &[("tran_type_cd", Some("0A")), ("tran_cat_cd", Some(""))],
    ),
    (
        "V-COTRN02C-021",
        "V-COTRN02C-022",
        messages::AMOUNT_SHOULD_BE_IN_FORMAT,
        &[
            ("tran_amt", Some("12.50")),
            ("tran_orig_date", Some("2022/07/01")),
        ],
    ),
    (
        "V-COTRN02C-024",
        "V-COTRN02C-026",
        messages::ORIG_DATE_NOT_A_VALID_DATE,
        &[
            ("tran_orig_date", Some("2022-02-30")),
            ("tran_merchant_id", Some("12345678A")),
        ],
    ),
    (
        "V-COTRN02C-025",
        "V-COTRN02C-028",
        messages::PROC_DATE_NOT_A_VALID_DATE,
        &[
            ("tran_proc_date", Some("2022-13-01")),
            ("confirm", Some("X")),
        ],
    ),
    (
        "V-COTRN02C-026",
        "V-COTRN02C-027",
        messages::MERCHANT_ID_MUST_BE_NUMERIC,
        &[
            ("tran_merchant_id", Some("12345678A")),
            ("confirm", Some("N")),
        ],
    ),
];

/// The amount entries the third `EVALUATE TRUE` refuses (P-COTRN02C-012),
/// each failing one tested position group of `TRNAMTI X(12)` alone: `(1:1)`
/// neither `-` nor `+`, `(2:8)` not digits, `(10:1)` not `.`, `(11:2)` not
/// digits, a short entry leaving byte 12 a space.
const AMOUNT_FORMAT_ENTRIES: [&str; 8] = [
    "000000012.34",
    " 00000012.34",
    "+0000001A.34",
    "-00 00012.34",
    "+000000012.3",
    "+00000012-34",
    "+00000012.3A",
    "+00000012.3",
];

/// Date entries the fourth and fifth `EVALUATE TRUE` refuse
/// (P-COTRN02C-013), each failing one tested position alone: `(1:4)`,
/// `(5:1)`, `(6:2)`, `(8:1)` and `(9:2)`.
const DATE_FORMAT_ENTRIES: [&str; 5] = [
    "20X2-07-01",
    "2022+07-01",
    "2022-0X-01",
    "2022-07/01",
    "2022-07-0X",
];

/// Amounts in the form `-99999999.99` and the exact value `NUMVAL-C` gives
/// each (P-COTRN02C-014), added with type `02`, which no reward rule names.
const NUMVAL_C_ENTRIES: [&str; 5] = [
    "-12345678.91",
    "+00000000.01",
    "-00000000.00",
    "-99999999.99",
    "+10000000.00",
];

/// The `COTRN2AI` input fields `INITIALIZE-ALL-FIELDS` clears
/// [app/cbl/COTRN02C.cbl:L762-779], in its order, each with the member of the
/// `POST /transactions` body that carries it.
const MAP_INPUT_FIELDS: [(&str, &str); 14] = [
    ("ACTIDINI", "account_id"),
    ("CARDNINI", "card_number"),
    ("TTYPCDI", "tran_type_cd"),
    ("TCATCDI", "tran_cat_cd"),
    ("TRNSRCI", "tran_source"),
    ("TRNAMTI", "tran_amt"),
    ("TDESCI", "tran_desc"),
    ("TORIGDTI", "tran_orig_date"),
    ("TPROCDTI", "tran_proc_date"),
    ("MIDI", "tran_merchant_id"),
    ("MNAMEI", "tran_merchant_name"),
    ("MCITYI", "tran_merchant_city"),
    ("MZIPI", "tran_merchant_zip"),
    ("CONFIRMI", "confirm"),
];

/// The `Expected result` keys of the `COTRN02` header fields P-COTRN02C-033
/// records as fixed texts: the two titles, quoted, and the transaction and
/// program names.
const HEADER_KEYS: [&str; 4] = ["title01", "title02", "trnname", "pgmname"];

/// The `code` of a 401 for a request without a valid bearer token.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// One date of each class CT02 reaches `CSUTLDTC` with, in the `YYYY-MM-DD`
/// form it passes: valid, before the supported range, a day beyond its month,
/// a month above 12 and a non-digit month.
const CSUTLDTC_DATES: [&str; 5] = [
    "2022-07-18",
    "1500-01-01",
    "2022-02-30",
    "2022-13-01",
    "2022-0A-18",
];

/// Dates `CEEDAYS` finds non-numeric or not matching the picture
/// (P-CSUTLDTC-006): a non-digit under `M`, under `Y` and under `D`, other
/// separators, and a blank field.
const NONNUMERIC_DATES: [&str; 5] = [
    "2022-0A-18",
    "20A2-07-18",
    "2022-07-1 ",
    "2022/07/18",
    "          ",
];

/// Dates of the classes held at halt (P-CSUTLDTC-009, E-31): year `0000`, and
/// dates outside 1582-10-15 to 9999-12-31 that are not calendar dates.
const HALT_DATES: [&str; 5] = [
    "0000-01-01",
    "0000-13-45",
    "1500-02-30",
    "1582-10-00",
    "9999-12-32",
];

/// Calendar dates outside the supported range with a year of 0001 or later,
/// which the halt row leaves accepted as 2513 (P-CSUTLDTC-009).
const OUT_OF_RANGE_DATES: [&str; 3] = ["1582-10-14", "1500-01-01", "0001-01-01"];

/// The years of the sweep that shows the P-CSUTLDTC-007 tokens unreachable:
/// the first and last supported years, a leap century, a common year and a
/// year before the range.
const SWEEP_YEARS: [&str; 5] = ["1582", "2000", "2023", "9999", "1500"];

/// Hides the cross-reference table: its reads then fail on the server with
/// SQLSTATE `42P01`.
const HIDE_XREF: &str = "ALTER TABLE card_xref RENAME TO card_xref_offline";

/// Puts the cross-reference table back.
const RESTORE_XREF: &str = "ALTER TABLE card_xref_offline RENAME TO card_xref";

/// Hides the live-master view the TRAN-ID allocation reads.
const HIDE_MASTER: &str = "ALTER VIEW transaction_master RENAME TO transaction_master_offline";

/// Puts the live-master view back.
const RESTORE_MASTER: &str = "ALTER VIEW transaction_master_offline RENAME TO transaction_master";

/// A test-only trigger function that gives the TRAN-ID of the row being
/// inserted to another live row first, so the insert itself meets the live key
/// it was allocated: the `DUPREC` of the `WRITE`. The depth test keeps the
/// trigger from firing for the row it inserts.
const TAKE_TRAN_ID_FUNCTION: &str = "CREATE FUNCTION test_take_tran_id() RETURNS trigger \
     LANGUAGE plpgsql AS $$ BEGIN \
     IF pg_trigger_depth() = 1 THEN \
     INSERT INTO transactions (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, \
         tran_amt, tran_merchant_id, tran_merchant_name, tran_merchant_city, \
         tran_merchant_zip, tran_card_num, tran_orig_ts, tran_proc_ts) \
     VALUES (NEW.tran_id, NEW.tran_type_cd, NEW.tran_cat_cd, NEW.tran_source, NEW.tran_desc, \
         NEW.tran_amt, NEW.tran_merchant_id, NEW.tran_merchant_name, NEW.tran_merchant_city, \
         NEW.tran_merchant_zip, NEW.tran_card_num, NEW.tran_orig_ts, NEW.tran_proc_ts); \
     END IF; RETURN NEW; END $$";

/// Installs [`TAKE_TRAN_ID_FUNCTION`] on every transaction insert.
const TAKE_TRAN_ID_TRIGGER: &str = "CREATE TRIGGER test_take_tran_id \
     BEFORE INSERT ON transactions FOR EACH ROW EXECUTE FUNCTION test_take_tran_id()";

/// Removes [`TAKE_TRAN_ID_TRIGGER`] and its function.
const DROP_TAKE_TRAN_ID: [&str; 2] = [
    "DROP TRIGGER test_take_tran_id ON transactions",
    "DROP FUNCTION test_take_tran_id()",
];

/// A test-only trigger function that fails every transaction insert.
const FAIL_WRITE_FUNCTION: &str = "CREATE FUNCTION test_fail_write() RETURNS trigger \
     LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test-only write failure'; END $$";

/// Installs [`FAIL_WRITE_FUNCTION`] on every transaction insert.
const FAIL_WRITE_TRIGGER: &str = "CREATE TRIGGER test_fail_write \
     BEFORE INSERT ON transactions FOR EACH ROW EXECUTE FUNCTION test_fail_write()";

/// Removes [`FAIL_WRITE_TRIGGER`] and its function.
const DROP_FAIL_WRITE: [&str; 2] = [
    "DROP TRIGGER test_fail_write ON transactions",
    "DROP FUNCTION test_fail_write()",
];

/// The test-only trigger function that fails every accrual insert.
const FAIL_ACCRUAL_FUNCTION: &str = "CREATE FUNCTION test_fail_accrual() RETURNS trigger \
     LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test-only accrual failure'; END $$";

/// Installs [`FAIL_ACCRUAL_FUNCTION`] on every accrual insert.
const FAIL_ACCRUAL_TRIGGER: &str = "CREATE TRIGGER test_fail_accrual \
     BEFORE INSERT ON reward_accruals FOR EACH ROW EXECUTE FUNCTION test_fail_accrual()";

/// Removes [`FAIL_ACCRUAL_TRIGGER`].
const DROP_FAIL_ACCRUAL: &str = "DROP TRIGGER test_fail_accrual ON reward_accruals";

/// One injected I/O failure: the rows it proves and the message they share,
/// the statements that inject and remove it, and the changes from
/// [`BASELINE`] that reach it.
struct Fault {
    ids: &'static [&'static str],
    message: &'static str,
    inject: &'static [&'static str],
    restore: &'static [&'static str],
    changes: &'static [Change],
}

/// The I/O failure rows of `COTRN02C`, in the order the add reaches them.
const FAULTS: [Fault; 5] = [
    // READ-CXACAIX-FILE WHEN OTHER (P-COTRN02C-008): the account-keyed read.
    Fault {
        ids: &["V-COTRN02C-005"],
        message: messages::UNABLE_TO_LOOKUP_ACCT_IN_XREF_AIX_FILE,
        inject: &[HIDE_XREF],
        restore: &[RESTORE_XREF],
        changes: &[],
    },
    // READ-CCXREF-FILE WHEN OTHER (P-COTRN02C-009): the card-keyed read.
    Fault {
        ids: &["V-COTRN02C-007"],
        message: messages::UNABLE_TO_LOOKUP_CARD_IN_XREF_FILE,
        inject: &[HIDE_XREF],
        restore: &[RESTORE_XREF],
        changes: &[("account_id", None), ("card_number", Some(CARD_NUMBER))],
    },
    // The STARTBR and READPREV failures show one text (P-COTRN02C-021,
    // P-COTRN02C-022), and the one highest-key read of the allocation stands
    // for both (P-COTRN02C-034), so a failure of that read proves both rows.
    Fault {
        ids: &["V-COTRN02C-030", "V-COTRN02C-031"],
        message: messages::UNABLE_TO_LOOKUP_TRANSACTION,
        inject: &[HIDE_MASTER],
        restore: &[RESTORE_MASTER],
        changes: &[],
    },
    // WRITE-TRANSACT-FILE DUPKEY or DUPREC (P-COTRN02C-025).
    Fault {
        ids: &["V-COTRN02C-032"],
        message: messages::TRAN_ID_ALREADY_EXIST,
        inject: &[TAKE_TRAN_ID_FUNCTION, TAKE_TRAN_ID_TRIGGER],
        restore: &DROP_TAKE_TRAN_ID,
        changes: &[],
    },
    // WRITE-TRANSACT-FILE WHEN OTHER (P-COTRN02C-025).
    Fault {
        ids: &["V-COTRN02C-033"],
        message: messages::UNABLE_TO_ADD_TRANSACTION,
        inject: &[FAIL_WRITE_FUNCTION, FAIL_WRITE_TRIGGER],
        restore: &DROP_FAIL_WRITE,
        changes: &[],
    },
];

/// The `code` of the error body for each `kind=` a validation row names: the
/// `ErrorBody::code` values `carddemo_api::error` publishes. The table is
/// written out here, not taken from the domain's error kinds, so the test
/// checks the published wire codes independently of the mapping it tests.
const KIND_CODES: [(&str, &str); 5] = [
    ("Invalid", "INVALID"),
    ("Empty", "EMPTY"),
    ("NotFound", "NOT_FOUND"),
    ("Duplicate", "DUPLICATE"),
    ("Unexpected", "UNEXPECTED"),
];

/// The error-body `code` of the audit kind `kind`.
fn kind_code(kind: &str) -> &'static str {
    let Some((_, code)) = KIND_CODES.iter().find(|(name, _)| *name == kind) else {
        panic!("no error code is published for the audit kind {kind:?}");
    };
    code
}

/// The HTTP status `text` names in the cell of audit row `id`.
fn parse_status(text: &str, id: &str) -> StatusCode {
    text.parse::<u16>()
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or_else(|| panic!("{id} assigns no HTTP status: {text:?}"))
}

/// The string of an audit code span written as a double-quoted text.
fn unquote(text: &str) -> Option<&str> {
    text.strip_prefix('"')?.strip_suffix('"')
}

/// The decimal value of `text`, an amount, a rate or a point count.
fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap_or_else(|error| panic!("{text:?} is no decimal: {error}"))
}

/// The trimmed cell `name` of validation row `id`.
fn cell<'a>(doc: &'a Document, id: &str, name: &str) -> &'a str {
    let Some((table, row)) = doc.find_row(id) else {
        panic!("the validation audit defines no {id} row");
    };
    table
        .cell(row, name)
        .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
        .trim()
}

/// `(Order, ID)` of validation row `id`: the order in which the COBOL reaches
/// the rows.
fn rank(doc: &Document, id: &str) -> (u16, String) {
    let order = cell(doc, id, "Order");
    let order = order
        .parse()
        .unwrap_or_else(|error| panic!("{id} has no numeric Order {order:?}: {error}"));
    (order, id.to_owned())
}

/// The answer a validation row gives.
struct Audited {
    status: StatusCode,
    code: &'static str,
    field: String,
}

/// The answer of validation row `id`, once its quoted text is `message`.
fn audited(doc: &Document, id: &str, message: &str) -> Audited {
    let quoted = code_span(cell(doc, id, "Message text")).map(|span| span.text);
    assert_eq!(
        quoted.as_deref().and_then(unquote),
        Some(message),
        "{id}: the messages constant is the audited text"
    );
    let trigger = cell(doc, id, "Trigger condition");
    let Some(kind) = key_values(trigger)
        .into_iter()
        .rev()
        .find(|pair| pair.key == "kind")
    else {
        panic!("{id} names no kind: {trigger}");
    };
    Audited {
        status: parse_status(cell(doc, id, "Status"), id),
        code: kind_code(&kind.value),
        field: cell(doc, id, "BMS field").to_owned(),
    }
}

/// The success answer of P-COTRN02C-024: its status, and the quoted pieces of
/// the text that `STRING` builds around the TRAN-ID.
struct Success {
    status: StatusCode,
    head: String,
    middle: String,
    tail: String,
}

impl Success {
    /// The success text of the add that was given `tran_id`. The TRAN-ID holds
    /// no space, so `DELIMITED BY SPACE` takes all 16 digits.
    fn text(&self, tran_id: &str) -> String {
        [self.head.as_str(), &self.middle, tran_id, &self.tail].concat()
    }
}

/// The success answer P-COTRN02C-024 records in `doc`, the parity audit.
fn success_answer(doc: &Document) -> Success {
    let id = "P-COTRN02C-024";
    let Some((table, row)) = doc.find_row(id) else {
        panic!("the parity audit defines no {id} row");
    };
    let result = table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"));
    let pairs = key_values(result);
    let value = |key: &str| {
        let Some(pair) = pairs.iter().find(|pair| pair.key == key) else {
            panic!("{id} gives no `{key}=`: {result}");
        };
        pair.value.as_str()
    };
    let pieces: Vec<String> = cell_code_spans(value("message"))
        .iter()
        .map(|span| {
            unquote(&span.text)
                .unwrap_or_else(|| panic!("{id} quotes no text in {:?}", span.text))
                .to_owned()
        })
        .collect();
    let Ok([head, middle, tail]) = <[String; 3]>::try_from(pieces) else {
        panic!("{id} builds its message from three quoted pieces: {result}");
    };
    Success {
        status: parse_status(value("result"), id),
        head,
        middle,
        tail,
    }
}

/// The cell `name` of the parity row `id`, once every review of the row in
/// `doc`, the parity audit, passed.
fn reviewed_cell(doc: &Document, id: &str, name: &str) -> String {
    let mut verdicts = Vec::new();
    for table in doc.tables().iter().filter(|table| table.is_review_log()) {
        for row in &table.rows {
            if table.cell(row, "Row ID") == Some(id) {
                verdicts.push(table.cell(row, "Verdict").unwrap_or_default());
            }
        }
    }
    assert!(
        !verdicts.is_empty() && verdicts.iter().all(|verdict| *verdict == "pass"),
        "{id} must have passed its review, found {verdicts:?}"
    );
    let Some((table, row)) = doc.find_row(id) else {
        panic!("the parity audit defines no {id} row");
    };
    table
        .cell(row, name)
        .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
        .to_owned()
}

/// The `Expected result` of the reviewed parity row `id`, once it records
/// every text of `recorded`.
fn reviewed_result(doc: &Document, id: &str, recorded: &[&str]) -> String {
    let result = reviewed_cell(doc, id, "Expected result");
    for text in recorded {
        assert!(result.contains(text), "{id} records no {text:?}: {result}");
    }
    result
}

/// The member names of the JSON object `body`, sorted.
fn member_names(body: &Value) -> Vec<&str> {
    let mut names: Vec<&str> = body
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    names.sort_unstable();
    names
}

/// The texts of the `COTRN02` header P-COTRN02C-033 records: each title
/// unquoted and trimmed, and the transaction and program names.
fn header_texts(parity: &Document) -> Vec<String> {
    let result = reviewed_result(
        parity,
        "P-COTRN02C-033",
        &["no API field carries the header"],
    );
    let pairs = key_values(&result);
    HEADER_KEYS
        .iter()
        .map(|key| {
            let Some(pair) = pairs.iter().find(|pair| pair.key == *key) else {
                panic!("P-COTRN02C-033 records no {key}: {result}");
            };
            let value = pair.value.as_str();
            code_span(value).map_or_else(
                || value.to_owned(),
                |span| span.text.trim_matches('"').trim().to_owned(),
            )
        })
        .collect()
}

/// The body of [`BASELINE`] with `changes` applied in order.
fn body(changes: &[Change]) -> Value {
    let mut members: Map<String, Value> = BASELINE
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Value::from(*value)))
        .collect();
    for (name, value) in changes {
        if let Some(value) = value {
            members.insert((*name).to_owned(), Value::from(*value));
        } else {
            members.remove(*name);
        }
    }
    Value::Object(members)
}

/// The string member `name` of the request `body`.
fn member<'a>(body: &'a Value, name: &str) -> &'a str {
    body[name]
        .as_str()
        .unwrap_or_else(|| panic!("the add body has no string `{name}`: {body}"))
}

/// One input of a `V-COTRN02C` row: the row, the message it carries and the
/// changes from [`BASELINE`].
struct Case {
    id: &'static str,
    message: &'static str,
    changes: Vec<Change>,
}

fn case(id: &'static str, message: &'static str, changes: &[Change]) -> Case {
    Case {
        id,
        message,
        changes: changes.to_vec(),
    }
}

/// A confirmed add on the seeded, empty live master, by account and by card:
/// `STARTBR` at `HIGH-VALUES` answers `NOTFND` and nothing is added
/// (P-COTRN02C-021, E-27).
fn empty_master_cases() -> Vec<Case> {
    vec![
        case("V-COTRN02C-029", messages::TRANSACTION_ID_NOT_FOUND, &[]),
        case(
            "V-COTRN02C-029",
            messages::TRANSACTION_ID_NOT_FOUND,
            &[
                ("account_id", None),
                ("card_number", Some(CARD_NUMBER)),
                ("confirm", Some("y")),
            ],
        ),
    ]
}

/// `VALIDATE-INPUT-KEY-FIELDS` and its cross-reference reads (P-COTRN02C-005
/// to P-COTRN02C-009): the account ID is tested before the card number, which
/// is read only when no account ID is entered.
fn key_cases() -> Vec<Case> {
    let mut cases = vec![
        case(
            "V-COTRN02C-001",
            messages::ACCOUNT_ID_MUST_BE_NUMERIC,
            &[("account_id", Some("0000000000A"))],
        ),
        case(
            "V-COTRN02C-001",
            messages::ACCOUNT_ID_MUST_BE_NUMERIC,
            &[("account_id", Some("50"))],
        ),
        // A valid card number does not save a non-numeric account ID.
        case(
            "V-COTRN02C-001",
            messages::ACCOUNT_ID_MUST_BE_NUMERIC,
            &[
                ("account_id", Some("0000000000A")),
                ("card_number", Some(CARD_NUMBER)),
            ],
        ),
        case(
            "V-COTRN02C-002",
            messages::CARD_NUMBER_MUST_BE_NUMERIC,
            &[
                ("account_id", None),
                ("card_number", Some("050002445376574X")),
            ],
        ),
        case(
            "V-COTRN02C-002",
            messages::CARD_NUMBER_MUST_BE_NUMERIC,
            &[
                ("account_id", Some(" ")),
                ("card_number", Some("0500024453765")),
            ],
        ),
        // P-CVACT01Y-016: no cross-reference for the account.
        case(
            "V-COTRN02C-004",
            messages::ACCOUNT_ID_NOT_FOUND,
            &[("account_id", Some("99999999999"))],
        ),
        // P-CVACT02Y-010: no cross-reference for the card.
        case(
            "V-COTRN02C-006",
            messages::CARD_NUMBER_NOT_FOUND,
            &[
                ("account_id", None),
                ("card_number", Some("9999999999999999")),
            ],
        ),
    ];
    cases.extend(BLANKS.iter().map(|&blank| {
        case(
            "V-COTRN02C-003",
            messages::ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED,
            &[("account_id", blank), ("card_number", blank)],
        )
    }));
    cases
}

/// Each blank form of each data field, every other field valid.
fn blank_cases() -> Vec<Case> {
    BLANK_ROWS
        .iter()
        .flat_map(|&(id, name, message)| {
            BLANKS
                .iter()
                .map(move |&blank| case(id, message, &[(name, blank)]))
        })
        .collect()
}

/// [`EDIT_CASES`], each a single changed member.
fn edit_cases() -> Vec<Case> {
    EDIT_CASES
        .iter()
        .map(|&(id, name, entry, message)| case(id, message, &[(name, Some(entry))]))
        .collect()
}

/// The confirmation, reached once every edit has passed (P-COTRN02C-018):
/// `N`, `n` and each blank form ask for it, and so does a blank confirmation
/// after each of the [`ACCEPTED_DATES`] as either date.
fn confirm_cases() -> Vec<Case> {
    let message = messages::CONFIRM_TO_ADD_THIS_TRANSACTION;
    let mut cases: Vec<Case> = BLANKS
        .iter()
        .chain(&[Some("N"), Some("n")])
        .map(|&confirm| case("V-COTRN02C-027", message, &[("confirm", confirm)]))
        .collect();
    for name in ["tran_orig_date", "tran_proc_date"] {
        cases.extend(ACCEPTED_DATES.iter().map(|&date| {
            case(
                "V-COTRN02C-027",
                message,
                &[(name, Some(date)), ("confirm", Some(""))],
            )
        }));
    }
    cases
}

/// Every input edit of `COTRN02C` in the COBOL order: key fields, blank data
/// fields, class, format and date edits, then the confirmation.
fn validation_cases() -> Vec<Case> {
    let mut cases = key_cases();
    cases.extend(blank_cases());
    cases.extend(edit_cases());
    cases.extend(confirm_cases());
    cases
}

/// One reviewed `COTRN02C` row of the parity audit, the texts its `Expected
/// result` records, and the inputs that prove it, each answered as the
/// validation row of its case.
struct ParityCase {
    id: &'static str,
    recorded: &'static [&'static str],
    cases: Vec<Case>,
}

/// The changes from [`BASELINE`] that leave only `members` in the body: every
/// other map field is blank, as on a first entry, whose map is `LOW-VALUES`.
fn only(members: &[Change]) -> Vec<Change> {
    BASELINE
        .iter()
        .map(|&(name, _)| (name, None))
        .chain(members.iter().copied())
        .collect()
}

/// The parity rows of a confirmed add on the seeded, empty live master: the
/// `STARTBR` of the allocation finds nothing, the anomaly E-27 reproduced as
/// `COTRN02C` answers it (P-COTRN02C-029).
fn empty_master_parity() -> ParityCase {
    ParityCase {
        id: "P-COTRN02C-029",
        recorded: &["escalation=E-27", "reproduced per program"],
        cases: empty_master_cases(),
    }
}

/// The key-field parity rows on a live master:
///
/// - P-COTRN02C-001: a first entry with a preselected card is that card
///   number alone in `CARDNINI`, then `PROCESS-ENTER-KEY`: the key edit and
///   the card's cross-reference read, then the first data edit, a blank type.
/// - P-COTRN02C-007: neither key entered, in each blank form.
/// - P-COTRN02C-032: `RECEIVE MAP` is never tested, so a request that
///   carries no field at all still runs the ENTER edits.
/// - P-COTRN02C-002: no AID key reaches the add, whose body has no member for
///   one: a body naming one is answered as ENTER, never with the invalid-key
///   text.
fn key_parity() -> Vec<ParityCase> {
    let blank_keys = BLANKS.iter().map(|&blank| {
        case(
            "V-COTRN02C-003",
            messages::ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED,
            &[("account_id", blank), ("card_number", blank)],
        )
    });
    vec![
        ParityCase {
            id: "P-COTRN02C-001",
            recorded: &["card number copied to CARDNINI, PROCESS-ENTER-KEY"],
            cases: vec![
                case(
                    "V-COTRN02C-008",
                    messages::TYPE_CD_CAN_NOT_BE_EMPTY,
                    &only(&[("card_number", Some(CARD_NUMBER))]),
                ),
                case(
                    "V-COTRN02C-006",
                    messages::CARD_NUMBER_NOT_FOUND,
                    &only(&[("card_number", Some("9999999999999999"))]),
                ),
                case(
                    "V-COTRN02C-002",
                    messages::CARD_NUMBER_MUST_BE_NUMERIC,
                    &only(&[("card_number", Some("050002445376574X"))]),
                ),
            ],
        },
        ParityCase {
            id: "P-COTRN02C-007",
            recorded: &[messages::ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED],
            cases: blank_keys.collect(),
        },
        ParityCase {
            id: "P-COTRN02C-032",
            recorded: &[
                "the RESP is never tested",
                "the JSON body of POST /transactions",
            ],
            cases: vec![case(
                "V-COTRN02C-003",
                messages::ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED,
                &only(&[]),
            )],
        },
        ParityCase {
            id: "P-COTRN02C-002",
            recorded: &[messages::CCDA_MSG_INVALID_KEY],
            cases: vec![
                case(
                    "V-COTRN02C-027",
                    messages::CONFIRM_TO_ADD_THIS_TRANSACTION,
                    &[("aid", Some("PF9")), ("confirm", Some(""))],
                ),
                case(
                    "V-COTRN02C-008",
                    messages::TYPE_CD_CAN_NOT_BE_EMPTY,
                    &[("aid", Some("CLEAR")), ("tran_type_cd", None)],
                ),
            ],
        },
    ]
}

/// The format parity rows of the data fields on a live master:
///
/// - P-COTRN02C-012: each entry of [`AMOUNT_FORMAT_ENTRIES`].
/// - P-COTRN02C-013: each entry of [`DATE_FORMAT_ENTRIES`] as either date;
///   both dates malformed answer for the origination date; and both format
///   edits come before either `CSUTLDTC` call, so a processing date out of
///   form answers before an origination date that is no calendar date.
fn format_parity() -> Vec<ParityCase> {
    let amounts = AMOUNT_FORMAT_ENTRIES.iter().map(|&entry| {
        case(
            "V-COTRN02C-021",
            messages::AMOUNT_SHOULD_BE_IN_FORMAT,
            &[("tran_amt", Some(entry))],
        )
    });
    let mut dates: Vec<Case> = DATE_FORMAT_ENTRIES
        .iter()
        .flat_map(|&entry| {
            [
                case(
                    "V-COTRN02C-022",
                    messages::ORIG_DATE_SHOULD_BE_IN_FORMAT,
                    &[("tran_orig_date", Some(entry))],
                ),
                case(
                    "V-COTRN02C-023",
                    messages::PROC_DATE_SHOULD_BE_IN_FORMAT,
                    &[("tran_proc_date", Some(entry))],
                ),
            ]
        })
        .collect();
    dates.push(case(
        "V-COTRN02C-022",
        messages::ORIG_DATE_SHOULD_BE_IN_FORMAT,
        &[
            ("tran_orig_date", Some("2022/07/01")),
            ("tran_proc_date", Some("2022/07/01")),
        ],
    ));
    dates.push(case(
        "V-COTRN02C-023",
        messages::PROC_DATE_SHOULD_BE_IN_FORMAT,
        &[
            ("tran_orig_date", Some("2022-02-30")),
            ("tran_proc_date", Some("2022/07/01")),
        ],
    ));
    vec![
        ParityCase {
            id: "P-COTRN02C-012",
            recorded: &[messages::AMOUNT_SHOULD_BE_IN_FORMAT],
            cases: amounts.collect(),
        },
        ParityCase {
            id: "P-COTRN02C-013",
            recorded: &[
                messages::ORIG_DATE_SHOULD_BE_IN_FORMAT,
                messages::PROC_DATE_SHOULD_BE_IN_FORMAT,
            ],
            cases: dates,
        },
    ]
}

/// Asserts that the parity row of `parity_case` passed its review in
/// `parity` and records its texts, then sends each of its inputs with `token`
/// as [`assert_rejected`] does: `before` is the [`write_state`] ahead of them.
async fn assert_parity_case(
    app: &TestApp,
    token: &str,
    audits: (&Document, &Document),
    parity_case: &ParityCase,
    before: &[Vec<Option<String>>],
) {
    let (validation, parity) = audits;
    reviewed_result(parity, parity_case.id, parity_case.recorded);
    assert!(
        !parity_case.cases.is_empty(),
        "{} has inputs",
        parity_case.id
    );
    for case in &parity_case.cases {
        assert_rejected(app, token, validation, case, before).await;
    }
}

/// The state a rejected or rolled-back add leaves unchanged: the counts of
/// [`WRITE_COUNTS`], then the rows of [`REWARD_BALANCES`] and of
/// [`ACCOUNT_BALANCES`].
async fn write_state(app: &TestApp) -> Vec<Vec<Option<String>>> {
    let stack = app.stack();
    let mut rows = stack.fetch_text_rows(WRITE_COUNTS).await;
    rows.extend(stack.fetch_text_rows(REWARD_BALANCES).await);
    rows.extend(stack.fetch_text_rows(ACCOUNT_BALANCES).await);
    rows
}

/// Sends the add of `case` with `token` and asserts the answer of its row, and
/// that the add wrote nothing: `before` is the [`write_state`] ahead of it.
async fn assert_rejected(
    app: &TestApp,
    token: &str,
    doc: &Document,
    case: &Case,
    before: &[Vec<Option<String>>],
) {
    let row = audited(doc, case.id, case.message);
    let resp = app.post(ROUTE, Some(token), &body(&case.changes)).await;
    assert_eq!(
        resp.0, row.status,
        "{} with {:?}: {}",
        case.id, case.changes, resp.2
    );
    assert_error(
        &resp,
        row.status,
        row.code,
        case.message,
        &[(row.field.as_str(), case.message)],
    );
    assert_eq!(
        write_state(app).await,
        before,
        "{} with {:?} wrote",
        case.id,
        case.changes
    );
}

/// Injects `fault`, sends its add with `token`, removes the fault, and asserts
/// the answer of each of its rows and that nothing was written: `before` is the
/// [`write_state`] ahead of it.
async fn assert_fault(
    app: &TestApp,
    token: &str,
    doc: &Document,
    fault: &Fault,
    before: &[Vec<Option<String>>],
) {
    let stack = app.stack();
    for statement in fault.inject {
        stack.exec_sql(statement).await;
    }
    let resp = app.post(ROUTE, Some(token), &body(fault.changes)).await;
    for statement in fault.restore {
        stack.exec_sql(statement).await;
    }
    for id in fault.ids {
        let row = audited(doc, id, fault.message);
        assert_eq!(resp.0, row.status, "{id}: {}", resp.2);
        assert_error(
            &resp,
            row.status,
            row.code,
            fault.message,
            &[(row.field.as_str(), fault.message)],
        );
    }
    assert_eq!(write_state(app).await, before, "{:?} wrote", fault.ids);
}

/// `ADD 1 TO WS-TRAN-ID-N`: the TRAN-ID after `tran_id` in the `9(16)` working
/// field, zero-padded to 16 digits (P-COTRN02C-020).
fn next_tran_id(tran_id: &str) -> String {
    let number: u64 = tran_id
        .parse()
        .unwrap_or_else(|error| panic!("{tran_id:?} is no TRAN-ID: {error}"));
    format!("{:016}", number + 1)
}

/// Whether the map field `name` of `body` is entered: present and neither
/// `SPACES` nor `LOW-VALUES`, the test `VALIDATE-INPUT-KEY-FIELDS` makes of
/// each key field (P-COTRN02C-005, P-COTRN02C-006). An empty member is spaces.
fn entered<'a>(body: &'a Value, name: &str) -> Option<&'a str> {
    body[name].as_str().filter(|value| {
        !(value.bytes().all(|byte| byte == b' ') || value.bytes().all(|byte| byte == 0))
    })
}

/// The card `MOVE CARDNINI TO TRAN-CARD-NUM` writes for the add of `body`
/// (P-COTRN02C-023, card from xref), from the cross-reference
/// `VALIDATE-INPUT-KEY-FIELDS` reads in `parity`'s fixture: an entered
/// account ID is tested first and its `CXACAIX` record gives the card
/// (P-COTRN02C-005); with the account ID blank, the `CCXREF` record of the
/// entered card number does (P-COTRN02C-006). The record is decoded from
/// `cardxref.txt` at its reviewed section A offsets (P-CVACT03Y-001,
/// P-CVACT03Y-003).
fn xref_card(body: &Value, parity: &Document) -> String {
    reviewed_result(parity, "P-COTRN02C-023", &["card from xref"]);
    let xref = if let Some(account_id) = entered(body, "account_id") {
        reviewed_result(
            parity,
            "P-COTRN02C-005",
            &["READ CXACAIX by account; card number taken from the xref record"],
        );
        fixture_records::account_xref(parity, account_id)
    } else {
        reviewed_result(parity, "P-COTRN02C-006", &["READ CCXREF by card"]);
        fixture_records::card_xref(parity, member(body, "card_number"))
    };
    xref.value("xref_card_num").to_owned()
}

/// The `CVTRA05Y` record an add of `body` writes as TRAN-ID `tran_id`
/// (P-COTRN02C-023): the entered codes, texts and merchant ID, the amount as
/// its exact two-place decimal (P-COTRN02C-028), the card of the
/// cross-reference the add reads, decoded through `parity` ([`xref_card`]),
/// and the entered dates as the timestamps, with no clock reading
/// (P-COTRN02C-030).
fn written(body: &Value, tran_id: &str, parity: &Document) -> Value {
    json!({
        "tran_id": tran_id,
        "tran_type_cd": member(body, "tran_type_cd"),
        "tran_cat_cd": member(body, "tran_cat_cd"),
        "tran_source": member(body, "tran_source"),
        "tran_desc": member(body, "tran_desc"),
        "tran_amt": decimal(member(body, "tran_amt")).to_string(),
        "tran_merchant_id": member(body, "tran_merchant_id"),
        "tran_merchant_name": member(body, "tran_merchant_name"),
        "tran_merchant_city": member(body, "tran_merchant_city"),
        "tran_merchant_zip": member(body, "tran_merchant_zip"),
        "tran_card_num": xref_card(body, parity),
        "tran_orig_ts": member(body, "tran_orig_date"),
        "tran_proc_ts": member(body, "tran_proc_date"),
    })
}

/// Sends the add of [`BASELINE`] with `changes` and asserts the success answer
/// of P-COTRN02C-024: the TRAN-ID `tran_id`, the success text and the record
/// written, its card read through `parity` ([`written`]), which CT01 then
/// reads back unchanged. Returns the TRAN-ID.
async fn add(
    app: &TestApp,
    token: &str,
    success: &Success,
    changes: &[Change],
    tran_id: &str,
    parity: &Document,
) -> String {
    let body = body(changes);
    let (status, answer, raw) = app.post(ROUTE, Some(token), &body).await;
    assert_eq!(status, success.status, "the add with {changes:?}: {raw}");
    let text = success.text(tran_id);
    assert_eq!(
        text,
        messages::transaction_added_message(tran_id),
        "the composed success text is the audited one"
    );
    assert_eq!(
        answer["message"].as_str(),
        Some(text.as_str()),
        "the success text of the add with {changes:?}: {raw}"
    );
    assert_eq!(
        answer["transaction"],
        written(&body, tran_id, parity),
        "the record of the add with {changes:?}: {raw}"
    );
    let (status, viewed, raw) = app.get(&format!("{ROUTE}/{tran_id}"), Some(token)).await;
    assert_eq!(status, StatusCode::OK, "CT01 of {tran_id}: {raw}");
    assert_eq!(
        viewed, answer["transaction"],
        "CT01 reads the record CT02 wrote: {raw}"
    );
    tran_id.to_owned()
}

/// The ID and rate of the seeded retail rule, seed rule 1 (AAP 0.3.6).
async fn retail_rule(app: &TestApp) -> (String, Decimal) {
    let rows = app.stack().fetch_text_rows(RETAIL_RULE).await;
    let [row] = rows.as_slice() else {
        panic!("one seeded rule for 01/0001 expected: {rows:?}");
    };
    let [Some(rule_id), Some(rate)] = row.as_slice() else {
        panic!("the retail rule has a rule ID and a rate: {row:?}");
    };
    assert_eq!(rule_id, "1", "the retail rule is the first seeded rule");
    (rule_id.clone(), decimal(rate))
}

/// The points of the add of [`BASELINE`] with `changes` under `rate`:
/// `ceil(amount × rate)`, the one rounding of the engine (AAP 0.3.5).
fn points_of(changes: &[Change], rate: Decimal) -> Decimal {
    (decimal(member(&body(changes), "tran_amt")) * rate).ceil()
}

/// The earned, redeemed and available points of account 1, all zero while it
/// has no balance row.
async fn account_rewards(app: &TestApp) -> [Decimal; 3] {
    let rows = app.stack().fetch_text_rows(REWARD_BALANCES).await;
    let Some(row) = rows
        .iter()
        .find(|row| row.first().and_then(Option::as_deref) == Some(ACCOUNT_KEY))
    else {
        return [Decimal::ZERO; 3];
    };
    [1, 2, 3].map(|column| {
        let text = row.get(column).and_then(Option::as_deref);
        decimal(text.unwrap_or_else(|| panic!("a reward balance column is null: {row:?}")))
    })
}

/// The accrual of the add given `tran_id`, as [`ACCRUALS`] reads it: rule
/// `rule_id`, account 1, `points`, at the processing clock.
fn accrual_row(tran_id: &str, rule_id: &str, points: Decimal) -> Vec<Option<String>> {
    [tran_id, rule_id, ACCOUNT_KEY, &points.to_string(), CLOCK]
        .map(|text| Some(text.to_owned()))
        .to_vec()
}

/// `FUNCTION NUMVAL-C` of an amount the format edit accepted, as the text of
/// the two-place decimal the add stores (P-COTRN02C-014): the eight integer
/// digits and the two decimals exact, the sign kept, and a negative zero zero.
fn numval_c(entry: &str) -> String {
    let digits: String = entry.chars().filter(char::is_ascii_digit).collect();
    let cents: u64 = digits
        .parse()
        .unwrap_or_else(|error| panic!("{entry:?} holds no digits: {error}"));
    let sign = if entry.starts_with('-') && cents > 0 {
        "-"
    } else {
        ""
    };
    format!("{sign}{}.{:02}", cents / 100, cents % 100)
}

/// Asserts that the 201 `answer` of an add is what follows
/// `INITIALIZE-ALL-FIELDS` after the write (P-COTRN02C-036): the record and
/// the success text, and no member that echoes the cleared input fields; and
/// that no text of it is one of the 3270 header's (P-COTRN02C-033).
fn assert_add_answer(answer: &Value, parity: &Document) {
    reviewed_result(
        parity,
        "P-COTRN02C-036",
        &["the form is sent empty with that text (P-COTRN02C-024)"],
    );
    assert_eq!(
        member_names(answer),
        ["message", "transaction"],
        "the members of the 201 answer: {answer}"
    );
    let headers = header_texts(parity);
    let texts = answer["transaction"]
        .as_object()
        .into_iter()
        .flat_map(|record| record.values())
        .chain([&answer["message"]])
        .filter_map(Value::as_str);
    for text in texts {
        assert!(
            !headers.iter().any(|header| header == text.trim()),
            "the 201 answer carries the header text {text:?}: {answer}"
        );
    }
}

/// Adds each amount of [`NUMVAL_C_ENTRIES`] with type `02` after the TRAN-ID
/// `previous`, and asserts that each stores the exact `NUMVAL-C` value
/// (P-COTRN02C-014), that CT01 reads it back, and that each answer is the
/// 201 of P-COTRN02C-024. Returns the last TRAN-ID added.
async fn numval_c_adds(
    app: &TestApp,
    token: &str,
    success: &Success,
    previous: &str,
    parity: &Document,
) -> String {
    reviewed_result(
        parity,
        "P-COTRN02C-014",
        &["NUMVAL-C of sign + 8 digits + point + 2 digits"],
    );
    let mut previous = previous.to_owned();
    for entry in NUMVAL_C_ENTRIES {
        let tran_id = next_tran_id(&previous);
        let body = body(&[("tran_type_cd", Some("02")), ("tran_amt", Some(entry))]);
        let (status, answer, raw) = app.post(ROUTE, Some(token), &body).await;
        assert_eq!(status, success.status, "the add of {entry}: {raw}");
        assert_eq!(
            answer["message"].as_str(),
            Some(success.text(&tran_id).as_str()),
            "the success text of the add of {entry}: {raw}"
        );
        let mut record = written(&body, &tran_id, parity);
        record["tran_amt"] = Value::from(numval_c(entry));
        assert_eq!(answer["transaction"], record, "the add of {entry}: {raw}");
        assert_add_answer(&answer, parity);
        let (status, viewed, raw) = app.get(&format!("{ROUTE}/{tran_id}"), Some(token)).await;
        assert_eq!(status, StatusCode::OK, "CT01 of {tran_id}: {raw}");
        assert_eq!(viewed, record, "CT01 reads the amount {entry} back: {raw}");
        previous = tran_id;
    }
    previous
}

/// PF5 reads the highest-keyed transaction of the whole live master, whatever
/// its account (P-COTRN02C-002, P-COTRN02C-032): the SPA's copy source is
/// `GET /transactions?direction=backward&page_size=1`, which answers the add
/// given `last`, not the higher-keyed record `batch tranbkp` took out of the
/// live master (E-09).
async fn copy_last_source(app: &TestApp, token: &str, last: &str, parity: &Document) {
    reviewed_result(parity, "P-COTRN02C-002", &["COPY-LAST-TRAN-DATA"]);
    reviewed_result(
        parity,
        "P-COTRN02C-032",
        &["GET /transactions?direction=backward&page_size=1 for the PF5 copy"],
    );
    let path = format!("{ROUTE}?direction=backward&page_size=1");
    let (status, page, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
    let (status, viewed, view_raw) = app.get(&format!("{ROUTE}/{last}"), Some(token)).await;
    assert_eq!(status, StatusCode::OK, "CT01 of {last}: {view_raw}");
    assert_eq!(page["items"], json!([viewed]), "the PF5 copy source: {raw}");
}

/// `EIBCALEN = 0` and PF3 transfer control without any file access
/// (P-COTRN02C-031): an add without a session is answered 401 before its
/// cross-reference read, so it stays 401 while that read fails, and writes
/// nothing; PF3 is the Back link, which sends no request. `before` is the
/// [`write_state`] ahead of it.
async fn signed_off_add(app: &TestApp, parity: &Document, before: &[Vec<Option<String>>]) {
    reviewed_result(
        parity,
        "P-COTRN02C-031",
        &["no file access", "no token gives 401"],
    );
    let stack = app.stack();
    stack.exec_sql(HIDE_XREF).await;
    let resp = app.post(ROUTE, None, &body(&[])).await;
    stack.exec_sql(RESTORE_XREF).await;
    assert_error(
        &resp,
        StatusCode::UNAUTHORIZED,
        UNAUTHENTICATED,
        messages::AUTHENTICATION_REQUIRED,
        &[],
    );
    assert_eq!(
        write_state(app).await,
        before,
        "an add without a session wrote"
    );
}

/// Every `V-COTRN02C` row is exercised by an input of this file, each with
/// the message constant its audited text quotes. Needs no container.
#[test]
fn every_cotrn02c_row_is_exercised_with_its_audited_text() {
    let doc = common::validation_audit();
    let mut exercised = BTreeSet::new();
    for case in empty_master_cases().iter().chain(&validation_cases()) {
        audited(&doc, case.id, case.message);
        exercised.insert(case.id.to_owned());
    }
    for (first, later, message, _) in PRECEDENCE {
        audited(&doc, first, message);
        assert!(
            exercised.contains(first) && exercised.contains(later),
            "{first} and {later} each have an input of their own"
        );
    }
    for fault in &FAULTS {
        for id in fault.ids {
            audited(&doc, id, fault.message);
            exercised.insert((*id).to_owned());
        }
    }
    let defined: BTreeSet<String> = common::validation_ids(PROGRAM).into_iter().collect();
    assert_eq!(
        exercised, defined,
        "the rows this file exercises are the {PROGRAM} rows of the audit"
    );
}

/// The keys of `COTRN02` as the API serves them (P-COTRN02C-002): ENTER is
/// the one `POST`, whose body is exactly the input fields
/// `INITIALIZE-ALL-FIELDS` clears (P-COTRN02C-036), so no member carries an
/// AID key and the invalid-key text has no request; PF5 is the list's
/// backward page of one row (P-COTRN02C-032); PF3, the Back link
/// (P-COTRN02C-031), and PF4, the Reset button (P-COTRN02C-035), send no
/// request, and no other operation exists. The 201 answer holds the record and
/// the success text, and no member for the 3270 header (P-COTRN02C-033).
/// Needs no container.
#[test]
fn ct02_keys_are_one_post_of_the_map_input_fields() {
    let parity = common::parity_audit();
    reviewed_result(&parity, "P-COTRN02C-002", &[messages::CCDA_MSG_INVALID_KEY]);
    reviewed_result(
        &parity,
        "P-COTRN02C-031",
        &["the Back link to /transactions"],
    );
    reviewed_result(
        &parity,
        "P-COTRN02C-035",
        &["no write", "the Reset button clears the form, no request"],
    );
    reviewed_result(
        &parity,
        "P-COTRN02C-036",
        &["the 11 data fields", "PF4 is the Reset button"],
    );
    reviewed_result(
        &parity,
        "P-COTRN02C-032",
        &["the JSON body of POST /transactions"],
    );
    header_texts(&parity);
    let cleared = reviewed_cell(&parity, "P-COTRN02C-036", "Copybook field or PIC clause");
    let fields: Vec<&str> = cleared
        .split(';')
        .nth(1)
        .map(|part| {
            part.trim()
                .trim_end_matches("(COTRN2AI)")
                .split(',')
                .map(str::trim)
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<&str> = MAP_INPUT_FIELDS.iter().map(|(name, _)| *name).collect();
    assert_eq!(fields, names, "the fields P-COTRN02C-036 clears: {cleared}");

    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the API document is no JSON: {error}"));
    let item = &spec["paths"][ROUTE];
    assert_eq!(
        member_names(item),
        ["get", "post"],
        "the operations of {ROUTE}: {item}"
    );
    let schemas = &spec["components"]["schemas"];
    let mut members: Vec<&str> = MAP_INPUT_FIELDS.iter().map(|(_, member)| *member).collect();
    members.sort_unstable();
    assert_eq!(
        member_names(&schemas["TransactionAddRequest"]["properties"]),
        members,
        "the members of the add body"
    );
    assert_eq!(
        item["post"]["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/TransactionAddRequest",
        "the body of the add: {item}"
    );
    assert_eq!(
        member_names(&schemas["TransactionAddResponse"]["properties"]),
        ["message", "transaction"],
        "the members of the 201 answer"
    );
    let parameters: BTreeSet<&str> = item["get"]["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|parameter| parameter["name"].as_str())
        .collect();
    assert!(
        parameters.is_superset(&BTreeSet::from(["direction", "page_size"])),
        "the PF5 copy source takes direction and page_size: {parameters:?}"
    );
}

/// The input edits answer in the COBOL order, each with its audited status,
/// code, text and BMS field, and none writes a transaction, an accrual or a
/// balance.
#[test]
fn ct02_answers_each_audited_edit_in_cobol_order_and_writes_nothing() {
    let doc = common::validation_audit();
    let parity = common::parity_audit();
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        assert_eq!(
            app.stack().fetch_text_rows(LIVE_COUNT).await,
            [[Some("0".to_owned())]],
            "the seeded live master is empty (E-07)"
        );
        let empty = write_state(&app).await;
        for case in empty_master_cases() {
            assert_rejected(&app, &token, &doc, &case, &empty).await;
        }
        let audits = (&doc, &parity);
        assert_parity_case(&app, &token, audits, &empty_master_parity(), &empty).await;
        // From here an accepted add would write, so each check below proves
        // that the rejection came before any write.
        app.stack().exec_sql(LIVE_ROWS).await;
        let before = write_state(&app).await;
        for case in validation_cases() {
            assert_rejected(&app, &token, &doc, &case, &before).await;
        }
        for parity_case in key_parity().iter().chain(&format_parity()) {
            assert_parity_case(&app, &token, audits, parity_case, &before).await;
        }
        for (first, later, message, changes) in PRECEDENCE {
            assert!(
                rank(&doc, first) < rank(&doc, later),
                "{first} ranks before {later} in the audit"
            );
            assert_rejected(&app, &token, &doc, &case(first, message, changes), &before).await;
        }
    });
}

/// A confirmed add is given the highest live TRAN-ID plus one, writes the
/// entered record, and accrues its points under the retail rule in the same
/// commit; the next adds count on from it, and those no rule rewards accrue
/// nothing. Their amounts are the exact `NUMVAL-C` values, and the last of
/// them is the PF5 copy source.
#[test]
fn ct02_adds_the_next_tran_id_and_accrues_its_points_with_the_insert() {
    let parity = common::parity_audit();
    let success = success_answer(&parity);
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let stack = app.stack();
        stack.exec_sql(LIVE_ROWS).await;
        let token = app.user_token().await;
        let (rule_id, rate) = retail_rule(&app).await;
        let [earned, redeemed, available] = account_rewards(&app).await;
        let account_before = stack.fetch_text_rows(ACCOUNT_BALANCES).await;

        let first = next_tran_id(HIGHEST_LIVE);
        add(&app, &token, &success, QUALIFYING, &first, &parity).await;
        let points = points_of(QUALIFYING, rate);
        let accrued = vec![accrual_row(&first, &rule_id, points)];
        assert_eq!(
            stack.fetch_text_rows(ACCRUALS).await,
            accrued,
            "one accrual for {first}"
        );
        let rewards = [earned + points, redeemed, available + points];
        assert_eq!(redeemed, Decimal::ZERO, "nothing is ever redeemed");
        assert_eq!(
            account_rewards(&app).await,
            rewards,
            "account 1 is credited"
        );

        assert_eq!(
            fixture_records::card_xref(&parity, CARD_NUMBER).value("xref_acct_id"),
            ACCOUNT_ID,
            "P-CVACT03Y-001, P-CVACT03Y-003: the card the card-keyed add enters cross-references \
             account 1, whose balance is checked below"
        );
        let mut previous = first;
        for changes in NON_QUALIFYING {
            let tran_id = next_tran_id(&previous);
            previous = add(&app, &token, &success, changes, &tran_id, &parity).await;
        }
        let last = numval_c_adds(&app, &token, &success, &previous, &parity).await;
        copy_last_source(&app, &token, &last, &parity).await;
        assert_eq!(
            stack.fetch_text_rows(ACCRUALS).await,
            accrued,
            "the adds no rule rewards accrue nothing"
        );
        assert_eq!(
            account_rewards(&app).await,
            rewards,
            "the balance is unchanged"
        );
        assert_eq!(
            stack.fetch_text_rows(ACCOUNT_BALANCES).await,
            account_before,
            "CT02 writes no account balance"
        );
    });
}

/// A failed accrual rolls the whole add back: the insert, the accrual and the
/// balance update, and the TRAN-ID allocation with them (AAP 0.3.5). The 500
/// carries the unexpected-error text and no internal detail.
#[test]
fn a_failed_accrual_rolls_the_ct02_add_back_whole() {
    let parity = common::parity_audit();
    let success = success_answer(&parity);
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let stack = app.stack();
        stack.exec_sql(LIVE_ROWS).await;
        stack.exec_sql(FAIL_ACCRUAL_FUNCTION).await;
        stack.exec_sql(FAIL_ACCRUAL_TRIGGER).await;
        let token = app.user_token().await;
        let before = write_state(&app).await;

        let resp = app.post(ROUTE, Some(&token), &body(QUALIFYING)).await;
        assert_error(
            &resp,
            StatusCode::INTERNAL_SERVER_ERROR,
            kind_code("Unexpected"),
            messages::UNEXPECTED_ERROR,
            &[],
        );
        assert_eq!(
            resp.1.as_object().map(Map::len),
            Some(2),
            "the 500 carries only `code` and `message`: {}",
            resp.2
        );
        assert_eq!(write_state(&app).await, before, "nothing persisted");

        // With accruals working again, the same TRAN-ID is given out and its
        // points accrue.
        stack.exec_sql(DROP_FAIL_ACCRUAL).await;
        let (rule_id, rate) = retail_rule(&app).await;
        let tran_id = next_tran_id(HIGHEST_LIVE);
        add(&app, &token, &success, QUALIFYING, &tran_id, &parity).await;
        assert_eq!(
            stack.fetch_text_rows(ACCRUALS).await,
            vec![accrual_row(&tran_id, &rule_id, points_of(QUALIFYING, rate))],
            "the retried add accrues"
        );
    });
}

/// Each audited I/O failure answers with its row's status, code, text and
/// BMS field and writes nothing; none of them uses up a TRAN-ID. An add
/// without a session is refused before any of those reads.
#[test]
fn ct02_answers_each_audited_io_failure_and_writes_nothing() {
    let doc = common::validation_audit();
    let parity = common::parity_audit();
    let success = success_answer(&parity);
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        app.stack().exec_sql(LIVE_ROWS).await;
        let token = app.user_token().await;
        let before = write_state(&app).await;
        for fault in &FAULTS {
            assert_fault(&app, &token, &doc, fault, &before).await;
        }
        signed_off_add(&app, &parity, &before).await;
        add(
            &app,
            &token,
            &success,
            &[],
            &next_tran_id(HIGHEST_LIVE),
            &parity,
        )
        .await;
    });
}

/// The ten bytes of the ASCII date `text`, as `COTRN02C` moves its `X(10)`
/// screen field to `CSUTLDTC-DATE`.
fn ten_bytes(text: &str) -> [u8; date_validation::LS_DATE_LEN] {
    text.as_bytes()
        .try_into()
        .unwrap_or_else(|_| panic!("{text:?} is not ten bytes"))
}

/// What `CSUTLDTC` returns for `text` under the CT02 picture `YYYY-MM-DD`,
/// for a class the audit pins.
fn csutldtc(text: &str) -> CsutldtcResult {
    date_validation::validate(&ten_bytes(text), DatePicture::YyyyMmDdDashed)
        .unwrap_or_else(|error| panic!("{text:?} has no pinned CEEDAYS feedback: {error}"))
}

/// The numbers that follow each `prefix` in `text`.
fn numbers_after(text: &str, prefix: &str) -> Vec<usize> {
    text.match_indices(prefix)
        .filter_map(|(at, _)| {
            let digits: String = text
                .get(at + prefix.len()..)
                .unwrap_or_default()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        })
        .collect()
}

/// The value of `key` in the `Expected result` `result` of the row `id`, up
/// to its first space: `0003` of `sev=0003`, `2520` of `msg=2520 (X'09D8')`.
fn result_value<'a>(result: &'a str, id: &str, key: &str) -> &'a str {
    let at = result
        .find(&format!("{key}="))
        .unwrap_or_else(|| panic!("{id} records no {key}=: {result}"));
    result
        .get(at + key.len() + 1..)
        .and_then(|rest| rest.split([' ', ';']).next())
        .unwrap_or_default()
}

/// The 80-byte `LS-RESULT` P-CSUTLDTC-008 lays out for the call that
/// answered `result` for `date`: severity, `Mesg Code:` and a space, message
/// number, space, the 15-byte text, space, `TstDate:` and a space, the ten
/// bytes X'000A' and the first eight date bytes (E-76), space, `Mask used:`,
/// the picture, four spaces.
fn expected_result_line(result: &CsutldtcResult, date: &str) -> String {
    let feedback = result.feedback();
    let head = date
        .get(..8)
        .unwrap_or_else(|| panic!("{date:?} is no date"));
    format!(
        "{:04}{} {:04} {:<15} {} \0\n{head} {}YYYY-MM-DD    ",
        feedback.severity(),
        messages::MESG_CODE,
        feedback.msg_no(),
        feedback.result_text(),
        messages::TST_DATE,
        messages::MASK_USED,
    )
}

/// The `CSUTLDTC` call and result CT02 relies on, rule by rule:
///
/// - P-CSUTLDTC-001: both arguments are ten-byte vstrings, the CT02 picture is
///   `YYYY-MM-DD` and the result is the 80-byte `WS-MESSAGE`.
/// - P-CSUTLDTC-008: the result is laid out field by field as the row writes
///   it; CT02 reads its severity at bytes 1-4 and its message number at bytes
///   16-19, and the severity is the `RETURN-CODE`.
/// - P-CSUTLDTC-010 (E-76): bytes 46-55 hold X'000A' and the first eight date
///   bytes, not the date moved there before the call.
///
/// Needs no container.
#[test]
fn csutldtc_returns_the_80_byte_result_ct02_reads() {
    let parity = common::parity_audit();
    let call = reviewed_result(&parity, "P-CSUTLDTC-001", &["call=CEEDAYS("]);
    let vstrings = numbers_after(&call, "vstring len ");
    assert_eq!(
        vstrings,
        [date_validation::LS_DATE_LEN; 2],
        "P-CSUTLDTC-001 passes a ten-byte date and picture: {call}"
    );
    assert_eq!(
        numbers_after(&call, "WS-MESSAGE "),
        [date_validation::RESULT_LEN],
        "P-CSUTLDTC-001 returns the 80-byte WS-MESSAGE: {call}"
    );
    assert_eq!(DatePicture::YyyyMmDdDashed.as_bytes(), b"YYYY-MM-DD");
    let layout = reviewed_result(
        &parity,
        "P-CSUTLDTC-008",
        &[
            "callers read only result(1:4) severity and result(16:4) message number",
            "RETURN-CODE=severity",
        ],
    );
    let labels: Vec<String> = cell_code_spans(&layout)
        .into_iter()
        .filter_map(|span| unquote(&span.text).map(str::to_owned))
        .collect();
    assert_eq!(
        labels,
        [messages::MESG_CODE, messages::TST_DATE, messages::MASK_USED],
        "the labels of the P-CSUTLDTC-008 layout: {layout}"
    );
    reviewed_result(
        &parity,
        "P-CSUTLDTC-010",
        &["escalation=E-76", "disposition=reproduced"],
    );
    for date in CSUTLDTC_DATES {
        let result = csutldtc(date);
        let line = result.result_line();
        assert_eq!(line.len(), date_validation::RESULT_LEN, "{date}");
        assert_eq!(line, expected_result_line(&result, date), "{date}");
        assert_eq!(
            line.get(..4),
            Some(result.severity_code().as_str()),
            "{date}"
        );
        assert_eq!(line.get(15..19), Some(result.msg_num().as_str()), "{date}");
        assert_eq!(
            result.severity_code().parse::<u16>().ok(),
            Some(result.feedback().severity()),
            "the RETURN-CODE of {date}"
        );
        let tst_date = line.as_bytes().get(45..55);
        assert_eq!(
            tst_date,
            Some(&[b"\0\n".as_slice(), &date.as_bytes()[..8]].concat()[..]),
            "TstDate of {date}"
        );
        assert_ne!(tst_date, Some(date.as_bytes()), "TstDate of {date}");
    }
}

/// The feedback classes of `CEEDAYS` beyond the ones CT02 accepts or rejects
/// by its edits:
///
/// - P-CSUTLDTC-006: a date that is not digits under its picture gets
///   severity 0003, message 2520 and the text the row quotes.
/// - P-CSUTLDTC-007: tokens 2509, 2518, 2521 and any other keep the numbers
///   and texts of the `EVALUATE`, and no CT02 date reaches them: a sweep of
///   every month 00 to 13 and day 00 to 32 of several years meets none.
/// - P-CSUTLDTC-009 (E-31, halt): year `0000` and out-of-range dates that are
///   not calendar dates get no guessed feedback but the escalation, while
///   out-of-range calendar dates with a year of 0001 or later are 2513, which
///   CT02 accepts.
///
/// Needs no container.
#[test]
fn csutldtc_answers_each_feedback_class_the_audit_pins() {
    let parity = common::parity_audit();
    let condition = reviewed_cell(&parity, "P-CSUTLDTC-006", "Input condition");
    assert!(
        condition.contains("CEE2EO: non-numeric data or date not matching picture"),
        "the class of P-CSUTLDTC-006: {condition}"
    );
    let nonnumeric = reviewed_result(&parity, "P-CSUTLDTC-006", &["sev=", "msg="]);
    let text = code_span(&nonnumeric)
        .unwrap_or_else(|| panic!("P-CSUTLDTC-006 quotes no text: {nonnumeric}"));
    let quoted = unquote(&text.text).unwrap_or_default();
    let width = text.width.unwrap_or_default();
    for date in NONNUMERIC_DATES {
        let result = csutldtc(date);
        assert_eq!(result.feedback(), CeedaysFeedback::NonNumericData, "{date}");
        let id = "P-CSUTLDTC-006";
        assert_eq!(result.severity_code(), result_value(&nonnumeric, id, "sev"));
        assert_eq!(result.msg_num(), result_value(&nonnumeric, id, "msg"));
        assert_eq!(
            result.result_line().get(20..20 + width),
            Some(format!("{quoted:<width$}").as_str()),
            "the result text of {date}"
        );
    }
    unreachable_tokens(&parity);
    let halt = reviewed_result(&parity, "P-CSUTLDTC-009", &["disposition=halt"]);
    let escalation = result_value(&halt, "P-CSUTLDTC-009", "escalation");
    for date in HALT_DATES {
        let outcome = date_validation::validate(&ten_bytes(date), DatePicture::YyyyMmDdDashed);
        assert!(
            matches!(
                outcome,
                Err(DateValidationError::Unproven { escalation_id }) if escalation_id == escalation
            ),
            "{date} is held at halt by {escalation}: {outcome:?}"
        );
    }
    for date in OUT_OF_RANGE_DATES {
        let result = csutldtc(date);
        assert_eq!(result.msg_num(), "2513", "{date}");
        assert!(result.accepts_with_2513(), "CT02 accepts {date}");
    }
}

/// P-CSUTLDTC-007: the numbers and texts of the tokens no CT02 date reaches,
/// read from the row, and a sweep of dashed digit dates that meets none of
/// them.
fn unreachable_tokens(parity: &Document) {
    let id = "P-CSUTLDTC-007";
    let row = reviewed_result(parity, id, &["unreachable from CT02/CR00"]);
    let texts: Vec<(String, Option<usize>)> = cell_code_spans(&row)
        .into_iter()
        .map(|span| {
            (
                unquote(&span.text).unwrap_or_default().to_owned(),
                span.width,
            )
        })
        .collect();
    let tokens = [
        CeedaysFeedback::InvalidEra,
        CeedaysFeedback::BadPicString,
        CeedaysFeedback::YearInEraZero,
        CeedaysFeedback::Other {
            severity: 3,
            msg_no: 2530,
        },
    ];
    assert_eq!(texts.len(), tokens.len(), "the texts of {id}: {row}");
    for (token, (text, width)) in tokens.iter().zip(&texts) {
        assert_eq!(token.result_text(), text.as_str(), "{token:?}");
        assert_eq!(*width, Some(15), "the width of {token:?}");
        if !matches!(token, CeedaysFeedback::Other { .. }) {
            let numbered = format!("{} `\"{text}\"`", token.msg_no());
            assert!(row.contains(&numbered), "{id} records {numbered}: {row}");
        }
    }
    for year in SWEEP_YEARS {
        for month in 0..=13_u8 {
            for day in 0..=32_u8 {
                let date = format!("{year}-{month:02}-{day:02}");
                let Ok(result) =
                    date_validation::validate(&ten_bytes(&date), DatePicture::YyyyMmDdDashed)
                else {
                    continue;
                };
                assert!(
                    !tokens.contains(&result.feedback()),
                    "{date} reaches {:?}",
                    result.feedback()
                );
            }
        }
    }
}
