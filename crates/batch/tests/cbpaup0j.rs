//! `batch cbpaup0j` parity: the purge of expired pending authorizations that the job
//! `CBPAUP0J` runs as the IMS BMP program `CBPAUP0C`
//! [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L24-37]
//! [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L136-180].
//!
//! The fixture corpus holds no pending authorization: the seed loads no IMS unload, so both
//! authorization tables start empty and the fixture expectations of the parity audit (section D)
//! have no CBPAUP0J subsection to derive. The test commits its own summaries and details
//! instead, under fixture accounts with their cross-reference customers and cards, and derives
//! every expected row and count from the reviewed section B, E and F rows of `CBPAUP0C`.
//!
//! [`PurgeRules`] reads each rule parameter from its audit cell, and panics with the cell's text
//! when the cell no longer states the clause it reads. [`PurgeRules::expect`] then applies these
//! rules to [`AUTHORIZATIONS`] in a test-local model that calls no production purge code, so a
//! changed audited rule changes what the test expects:
//!
//! - `1000-INITIALIZE` fills `PRM-INFO` from the audited `SYSIN` card by the audited field
//!   widths. It takes the expiry days when they are numeric, else the audited default. It
//!   applies the audited checkpoint-frequency defaults and the debug flag. It also takes
//!   `CURRENT-YYDDD` from the processing clock (P-CBPAUP0C-001, P-CBPAUP0C-012).
//! - `4000-CHECK-IF-EXPIRED` takes the authorization day as the audited complement minus
//!   `PA-AUTH-DATE-9C`. A detail expires when the day difference stands in the audited relation
//!   to the expiry days. Its approved count (audited response code) or declined count then falls
//!   in memory only, and the stored counts stay unchanged (P-CBPAUP0C-004, P-CBPAUP0C-011, E-35).
//! - The summary goes when the audited condition holds over its in-memory counts, a condition
//!   that as audited tests the approved count twice (E-16). It takes every detail still under
//!   it (P-CBPAUP0C-006, P-CBPAUP0C-007, P-CBPAUP0C-010).
//! - The sweep reads summaries in `ACCNTID` order and the details of each in twin order. It
//!   counts every summary and detail read, every expired-detail delete and every summary delete,
//!   but not a detail that goes with its summary (P-CBPAUP0C-002, P-CBPAUP0C-003,
//!   P-CBPAUP0C-005, P-CBPAUP0C-007).
//!
//! Guards then confirm that the derived outcome still shows E-16, E-35 and the cascade. The
//! final checkpoint commits every delete (P-CBPAUP0C-008), so the rows are read back after the
//! run from another connection and compared with the derived rows. The job's `DISPLAY` events
//! are compared member by member, as exact JSON value tokens, with the derived parameters, day
//! and counts.

pub mod common;

use std::fmt::Display;
use std::ops::Range;
use std::str::FromStr;

use carddemo_audit::Document;
use carddemo_domain::entities::{
    FraudFlag, MatchStatus, NewPendingAuthDetail, PA_ACCOUNT_STATUS_OCCURS, PendingAuthSummary,
};
use carddemo_domain::messages::CBPAUP0C_ABENDING;
use carddemo_domain::numeric::{
    AccountId, Alnum, AuthCount, CardNumber, CustomerId, Money, NumericCode,
};
use carddemo_domain::ports::AuthorizationStore;
use carddemo_persistence::Db;
use chrono::{DateTime, Datelike};
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
/// expiry 0 days, checkpoint frequency and checkpoint display frequency `00001`, debug `Y`. The
/// test checks it against the card P-CBPAUP0C-001 reads ([`PurgeRules::card`]).
const SYSIN: &str = "00,00001,00001,Y";

/// The processing clock the audit derives its batch expectations under, 2022-07-01T12:00:00Z,
/// handed to the binary as `CARDDEMO_CLOCK_OVERRIDE`. `ACCEPT CURRENT-YYDDD FROM DAY` gives
/// [`TODAY`] from it (P-CBPAUP0C-012).
const CLOCK: &str = TRANREPT_CLOCK;

/// `CURRENT-YYDDD` under [`CLOCK`], day 182 of 2022, which dates [`AUTHORIZATIONS`]. The test
/// checks it against [`current_yyddd`] of the clock before deriving anything from it.
const TODAY: i32 = 22_182;

/// The complement `COPAUA0C` `8500-INSERT-AUTH` stores the authorization date under:
/// `PA-AUTH-DATE-9C = 99999 - YYDDD`. The purge takes the date back with the complement that
/// P-CBPAUP0C-004 states ([`PurgeRules::complement`]).
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

/// The authorizations the test commits, in `ACCNTID` order, all against the clock's day 22182.
/// Each shows one audited rule as the audit now states it. The test derives what the purge keeps
/// and deletes from the audit ([`PurgeRules::expect`]), not from this list:
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

/// The event of `DISPLAY 'CBPAUP0C PARM RECEIVED :' PRM-INFO`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L192], whose fields are the `PRM-INFO`
/// receiver and the values `1000-INITIALIZE` derives from it ([`PurgeRules::parm_fields`]).
const PARM_EVENT: &str = "PARM RECEIVED";

/// The event of `DISPLAY 'TODAYS DATE            :' CURRENT-YYDDD`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L193], whose `today_yyddd` field is the
/// five-digit day the clock gives (P-CBPAUP0C-012).
const TODAY_EVENT: &str = "TODAYS DATE";

/// The event of the end-of-job `DISPLAY` lines
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L171-178], whose fields are the four
/// counts ([`EndOfJobCounts::fields`]).
const COUNTS_EVENT: &str = "expired pending authorizations purged";

/// The `Copybook field or PIC clause` column of the parity audit tables.
const COPYBOOK_FIELD: &str = "Copybook field or PIC clause";

/// The `Input condition` column of the parity audit tables.
const INPUT_CONDITION: &str = "Input condition";

/// The `Expected result` column of the parity audit tables.
const EXPECTED_RESULT: &str = "Expected result";

/// The width of the `FILLER X(01)` that follows each field of `PRM-INFO`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L98-108]. The field list of
/// P-CBPAUP0C-001 names only the four fields. `ACCEPT PRM-INFO FROM SYSIN` fills the item byte
/// by byte, so each comma of the card lands in one of these fillers.
const PRM_INFO_FILLER: usize = 1;

/// The text of the `column` cell of parity row `id` after the first `needle`.
///
/// # Panics
///
/// Panics, quoting the cell, if it no longer states `needle`.
fn clause<'d>(doc: &'d Document, id: &str, column: &str, needle: &str) -> &'d str {
    let cell = parity_cell(doc, id, column);
    let (_, rest) = cell
        .split_once(needle)
        .unwrap_or_else(|| panic!("{id} {column} no longer states {needle:?}: {cell}"));
    rest
}

/// The integer `text` spells, trimmed, for the clause `what` of parity row `id`.
///
/// # Panics
///
/// Panics, naming the row, if `text` is no integer of type `T`.
fn audit_int<T>(id: &str, what: &str, text: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    text.trim()
        .parse()
        .unwrap_or_else(|error| panic!("{id}: {what} {text:?} is no integer: {error}"))
}

/// A relational operator of an audited COBOL condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relation {
    /// `<`
    Less,
    /// `<=`
    LessOrEqual,
    /// `=`
    Equal,
    /// `>=`
    GreaterOrEqual,
    /// `>`
    Greater,
}

impl Relation {
    /// The operator `token` spells in parity row `id`.
    ///
    /// # Panics
    ///
    /// Panics, quoting `cell`, if `token` is no relational operator.
    fn parse(id: &str, token: &str, cell: &str) -> Self {
        match token {
            "<" => Self::Less,
            "<=" => Self::LessOrEqual,
            "=" => Self::Equal,
            ">=" => Self::GreaterOrEqual,
            ">" => Self::Greater,
            other => panic!("{id}: {other:?} is no relational operator: {cell}"),
        }
    }

    /// Whether `left` stands in this relation to `right`.
    const fn holds(self, left: i32, right: i32) -> bool {
        match self {
            Self::Less => left < right,
            Self::LessOrEqual => left <= right,
            Self::Equal => left == right,
            Self::GreaterOrEqual => left >= right,
            Self::Greater => left > right,
        }
    }
}

/// The in-memory counts of one summary while the sweep reads its details: `PA-APPROVED-AUTH-CNT`
/// and `PA-DECLINED-AUTH-CNT` of the segment `GN` read, less the expired details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MemoryCounts {
    /// `PA-APPROVED-AUTH-CNT`.
    approved: i32,
    /// `PA-DECLINED-AUTH-CNT`.
    declined: i32,
}

/// The summary count a term of the audited summary-delete condition tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CountField {
    /// `PA-APPROVED-AUTH-CNT`.
    Approved,
    /// `PA-DECLINED-AUTH-CNT`.
    Declined,
}

impl CountField {
    /// The count the data name `name` of parity row `id` designates.
    ///
    /// # Panics
    ///
    /// Panics, quoting `cell`, if `name` is neither summary count.
    fn parse(id: &str, name: &str, cell: &str) -> Self {
        match name {
            "PA-APPROVED-AUTH-CNT" => Self::Approved,
            "PA-DECLINED-AUTH-CNT" => Self::Declined,
            other => panic!("{id}: the condition tests {other:?}, no summary count: {cell}"),
        }
    }

    /// This count in `counts`.
    const fn of(self, counts: MemoryCounts) -> i32 {
        match self {
            Self::Approved => counts.approved,
            Self::Declined => counts.declined,
        }
    }
}

/// One `<count> <relation> <integer>` term of the audited summary-delete condition.
#[derive(Debug, Clone, Copy)]
struct Term {
    /// The count tested.
    field: CountField,
    /// The operator.
    relation: Relation,
    /// The literal the count is compared with.
    value: i32,
}

/// The audited summary-delete condition, as the `OR` of `AND` groups of [`Term`]s: COBOL
/// evaluates `AND` before `OR`.
#[derive(Debug)]
struct Condition {
    /// The groups, any of which deletes the summary when every term in it holds.
    any_of: Vec<Vec<Term>>,
}

impl Condition {
    /// The condition `text` of parity row `id`, terms joined by `AND` or `OR`.
    ///
    /// # Panics
    ///
    /// Panics, quoting `cell`, if a term is incomplete or a connective is neither `AND` nor
    /// `OR`.
    fn parse(id: &str, text: &str, cell: &str) -> Self {
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let mut any_of = Vec::new();
        let mut group = Vec::new();
        let mut rest = tokens.as_slice();
        loop {
            let [name, relation, value, tail @ ..] = rest else {
                panic!("{id}: an incomplete term {rest:?} in the condition: {cell}");
            };
            group.push(Term {
                field: CountField::parse(id, name, cell),
                relation: Relation::parse(id, relation, cell),
                value: audit_int(id, "the condition literal", value),
            });
            match tail {
                [] => break,
                ["AND", more @ ..] => rest = more,
                ["OR", more @ ..] => {
                    any_of.push(std::mem::take(&mut group));
                    rest = more;
                }
                [other, ..] => panic!("{id}: {other:?} joins no condition terms: {cell}"),
            }
        }
        any_of.push(group);
        Self { any_of }
    }

    /// Whether the condition holds over `counts`.
    fn holds(&self, counts: MemoryCounts) -> bool {
        self.any_of.iter().any(|group| {
            group
                .iter()
                .all(|term| term.relation.holds(term.field.of(counts), term.value))
        })
    }
}

/// `PRM-INFO` as `ACCEPT PRM-INFO FROM SYSIN` fills it from the `SYSIN` card of
/// P-CBPAUP0C-001, by the item widths that row lists.
#[derive(Debug)]
struct PrmInfo {
    /// The card.
    card: String,
    /// The receiver: the card padded with spaces to the audited items, each item followed by
    /// its [`PRM_INFO_FILLER`].
    text: String,
    /// The data name of each audited item and its bytes in `text`, in order.
    items: Vec<(String, Range<usize>)>,
}

impl PrmInfo {
    /// Reads the card and the `PRM-INFO` items of P-CBPAUP0C-001 and fills the receiver.
    ///
    /// # Panics
    ///
    /// Panics, quoting the cell, if the card is unterminated, an item has no `PIC` length, or
    /// the card does not fit the receiver.
    fn from_audit(doc: &Document) -> Self {
        const ID: &str = "P-CBPAUP0C-001";
        let card = clause(doc, ID, INPUT_CONDITION, "SYSIN '");
        let (card, _) = card
            .split_once('\'')
            .unwrap_or_else(|| panic!("{ID}: the SYSIN card is unterminated: {card}"));
        let cell = parity_cell(doc, ID, COPYBOOK_FIELD);
        let mut items = Vec::new();
        let mut len = 0;
        for item in clause(doc, ID, COPYBOOK_FIELD, "PRM-INFO:").split(',') {
            let (name, pic) = item
                .trim()
                .split_once(' ')
                .unwrap_or_else(|| panic!("{ID}: {item:?} has no PIC clause: {cell}"));
            let width: usize = pic
                .trim()
                .split_once('(')
                .and_then(|(_, length)| length.strip_suffix(')'))
                .and_then(|length| length.parse().ok())
                .unwrap_or_else(|| panic!("{ID}: {pic:?} gives no length: {cell}"));
            items.push((name.to_owned(), len..len + width));
            len += width + PRM_INFO_FILLER;
        }
        assert!(
            card.is_ascii() && card.len() <= len,
            "{ID}: the card {card:?} does not fit the {len}-byte PRM-INFO: {cell}"
        );
        Self {
            card: card.to_owned(),
            text: format!("{card:<len$}"),
            items,
        }
    }

    /// The text of the item `name`.
    ///
    /// # Panics
    ///
    /// Panics if P-CBPAUP0C-001 lists no such item.
    fn item(&self, name: &str) -> &str {
        let Some((_, bytes)) = self.items.iter().find(|(item, _)| item == name) else {
            panic!("P-CBPAUP0C-001 lists no {name} in PRM-INFO: {self:?}");
        };
        &self.text[bytes.clone()]
    }
}

/// A checkpoint frequency of `PRM-INFO` as `1000-INITIALIZE` leaves it: `default` when the item
/// is blank or zero, else the number it holds (P-CBPAUP0C-001).
///
/// # Panics
///
/// Panics if a non-blank, non-zero item is not a number, which the audit gives no value for.
fn frequency(item: &str, default: u32) -> u32 {
    let digits = item.trim_matches(|ch| ch == ' ' || ch == '\0');
    if digits.bytes().all(|byte| byte == b'0') {
        default
    } else {
        digits
            .parse()
            .unwrap_or_else(|error| panic!("checkpoint frequency {item:?}: {error}"))
    }
}

/// The `CBPAUP0C` purge as the reviewed section B rows state it, each parameter read from its
/// audit cell. Nothing here calls the production purge.
#[derive(Debug)]
struct PurgeRules {
    /// The `SYSIN` card of P-CBPAUP0C-001.
    card: String,
    /// `PRM-INFO` as `ACCEPT PRM-INFO FROM SYSIN` fills it from the card: the card padded with
    /// spaces to the audited item widths, each followed by its filler.
    prm_info: String,
    /// `WS-EXPIRY-DAYS`: `P-EXPIRY-DAYS` when numeric, else the audited default.
    expiry_days: i32,
    /// `P-CHKP-FREQ` after its audited default.
    chkp_freq: u32,
    /// `P-CHKP-DIS-FREQ` after its audited default.
    chkp_dis_freq: u32,
    /// `DEBUG-ON`: `P-DEBUG-FLAG` holds the audited flag.
    debug: bool,
    /// The minuend of `WS-AUTH-DATE = <complement> - PA-AUTH-DATE-9C` (P-CBPAUP0C-004).
    complement: i32,
    /// The relation of the day difference to the expiry days that expires a detail.
    expiry_relation: Relation,
    /// The `PA-AUTH-RESP-CODE` whose expiry decrements the approved count; any other code
    /// decrements the declined count.
    approved_code: String,
    /// The condition that deletes a summary after its details (P-CBPAUP0C-006).
    summary_deleted: Condition,
}

impl PurgeRules {
    /// Reads the rules from the `CBPAUP0C` rows of `doc`.
    ///
    /// # Panics
    ///
    /// Panics, quoting the cell, when a row no longer states a clause the derivation reads.
    fn from_audit(doc: &Document) -> Self {
        Self::check_sweep_counting(doc);
        let prm = PrmInfo::from_audit(doc);
        let expiry_days = Self::expiry_days(doc, prm.item("P-EXPIRY-DAYS"));
        let (chkp_default, display_default) = Self::frequency_defaults(doc);
        let chkp_freq = frequency(prm.item("P-CHKP-FREQ"), chkp_default);
        let chkp_dis_freq = frequency(prm.item("P-CHKP-DIS-FREQ"), display_default);
        let debug = prm.item("P-DEBUG-FLAG") == Self::debug_flag(doc);
        Self {
            card: prm.card,
            prm_info: prm.text,
            expiry_days,
            chkp_freq,
            chkp_dis_freq,
            debug,
            complement: Self::auth_date_complement(doc),
            expiry_relation: Self::expiry_relation(doc),
            approved_code: Self::approved_code(doc),
            summary_deleted: Self::summary_condition(doc),
        }
    }

    /// P-CBPAUP0C-001: `WS-EXPIRY-DAYS` from the `P-EXPIRY-DAYS` item `item`, which is used
    /// when numeric, else the audited default.
    fn expiry_days(doc: &Document, item: &str) -> i32 {
        const ID: &str = "P-CBPAUP0C-001";
        let rule = clause(doc, ID, EXPECTED_RESULT, "expiry days numeric -> used");
        let (_, fallback) = rule
            .split_once(", else ")
            .unwrap_or_else(|| panic!("{ID} gives no default expiry: {rule}"));
        let fallback: String = fallback.chars().take_while(char::is_ascii_digit).collect();
        if !item.is_empty() && item.bytes().all(|byte| byte.is_ascii_digit()) {
            audit_int(ID, "P-EXPIRY-DAYS", item)
        } else {
            audit_int(ID, "the default expiry", &fallback)
        }
    }

    /// P-CBPAUP0C-001: the defaults of the checkpoint frequency and of the display frequency.
    fn frequency_defaults(doc: &Document) -> (u32, u32) {
        const ID: &str = "P-CBPAUP0C-001";
        let rule = clause(
            doc,
            ID,
            EXPECTED_RESULT,
            "checkpoint frequency and display frequency default ",
        );
        let (chkp, display) = rule
            .split_once(" when blank or zero")
            .and_then(|(both, _)| both.split_once(" and "))
            .unwrap_or_else(|| panic!("{ID} gives no frequency defaults: {rule}"));
        (
            audit_int(ID, "the checkpoint frequency default", chkp),
            audit_int(ID, "the display frequency default", display),
        )
    }

    /// P-CBPAUP0C-001: the `P-DEBUG-FLAG` value that keeps the `DISPLAY` output.
    fn debug_flag(doc: &Document) -> &str {
        const ID: &str = "P-CBPAUP0C-001";
        let rule = clause(doc, ID, EXPECTED_RESULT, "debug flag ");
        let Some((flag, _)) = rule.split_once(" keeps DISPLAY output") else {
            panic!("{ID} gives no debug flag: {rule}");
        };
        flag
    }

    /// P-CBPAUP0C-004: the minuend of `WS-AUTH-DATE = <complement> - PA-AUTH-DATE-9C`.
    fn auth_date_complement(doc: &Document) -> i32 {
        const ID: &str = "P-CBPAUP0C-004";
        let field = clause(doc, ID, COPYBOOK_FIELD, "WS-AUTH-DATE");
        let Some((minuend, _)) = field
            .split_once('=')
            .and_then(|(_, sum)| sum.split_once(" - PA-AUTH-DATE-9C"))
        else {
            panic!("{ID} no longer computes WS-AUTH-DATE from PA-AUTH-DATE-9C: {field}");
        };
        audit_int(ID, "the WS-AUTH-DATE complement", minuend)
    }

    /// P-CBPAUP0C-004: the relation of the day difference `CURRENT-YYDDD - authorization
    /// YYDDD` to the expiry days that expires and deletes a detail, read from the operator in
    /// front of `expiry days`.
    fn expiry_relation(doc: &Document) -> Relation {
        const ID: &str = "P-CBPAUP0C-004";
        // The day difference `PurgeRules::expired` computes.
        clause(
            doc,
            ID,
            EXPECTED_RESULT,
            "day difference = CURRENT-YYDDD - authorization YYDDD",
        );
        let cell = parity_cell(doc, ID, EXPECTED_RESULT);
        let (before, _) = cell
            .split_once(" expiry days -> delete")
            .unwrap_or_else(|| panic!("{ID} no longer deletes by the expiry days: {cell}"));
        let operator = before
            .rsplit(char::is_whitespace)
            .next()
            .unwrap_or_default();
        Relation::parse(ID, operator, cell)
    }

    /// P-CBPAUP0C-004: the response code whose expiry decrements the approved count.
    fn approved_code(doc: &Document) -> String {
        const ID: &str = "P-CBPAUP0C-004";
        // The decrements stay in memory, so a kept summary keeps the counts and amounts it was
        // stored with, as `PurgeRules::expect` gives its row.
        clause(
            doc,
            ID,
            EXPECTED_RESULT,
            "the stored counts stay unchanged (E-35)",
        );
        let rule = clause(doc, ID, EXPECTED_RESULT, "in-memory approved (response '");
        let (code, _) = rule
            .split_once('\'')
            .unwrap_or_else(|| panic!("{ID}: the approved response code is unterminated: {rule}"));
        code.to_owned()
    }

    /// P-CBPAUP0C-006: the condition between `IF ` and `:` of the rule. The summary `DLET` also
    /// deletes every detail still under it, as `PurgeRules::expect` removes them.
    fn summary_condition(doc: &Document) -> Condition {
        const ID: &str = "P-CBPAUP0C-006";
        clause(
            doc,
            ID,
            EXPECTED_RESULT,
            "the summary DLET also deletes every detail still under it",
        );
        let rule = expected_value(doc, ID, "rule");
        let Some((condition, _)) = rule
            .strip_prefix("IF ")
            .and_then(|tested| tested.split_once(':'))
        else {
            panic!("{ID} states no IF condition: {rule}");
        };
        Condition::parse(ID, condition, &rule)
    }

    /// P-CBPAUP0C-002, P-CBPAUP0C-003, P-CBPAUP0C-005 and P-CBPAUP0C-007: the sweep reads the
    /// summaries in `ACCNTID` order and the details of each, and counts each delete of an
    /// expired detail and of a qualifying summary, as `PurgeRules::expect` counts them.
    fn check_sweep_counting(doc: &Document) {
        clause(
            doc,
            "P-CBPAUP0C-002",
            EXPECTED_RESULT,
            "next summary in ACCNTID order",
        );
        clause(
            doc,
            "P-CBPAUP0C-003",
            EXPECTED_RESULT,
            "next detail of the summary",
        );
        for (id, deleted) in [
            ("P-CBPAUP0C-005", "expired detail"),
            ("P-CBPAUP0C-007", "summary qualifies"),
        ] {
            clause(doc, id, INPUT_CONDITION, deleted);
            clause(doc, id, EXPECTED_RESULT, "DIBSTAT spaces -> counted");
        }
    }

    /// The fields of the [`PARM_EVENT`], each name with its JSON value token: the `PRM-INFO`
    /// receiver and the values `1000-INITIALIZE` derives from it (P-CBPAUP0C-001).
    fn parm_fields(&self) -> [(&'static str, String); 5] {
        [
            ("sysin", quoted(&self.prm_info)),
            ("expiry_days", self.expiry_days.to_string()),
            ("chkp_freq", self.chkp_freq.to_string()),
            ("chkp_dis_freq", self.chkp_dis_freq.to_string()),
            ("debug", self.debug.to_string()),
        ]
    }

    /// Whether the detail `input` expires on `today`: `4000-CHECK-IF-EXPIRED` takes its day back
    /// from the stored key with the audited complement, and compares the day difference with the
    /// expiry days by the audited relation (P-CBPAUP0C-004).
    fn expired(&self, input: DetailInput, today: i32) -> bool {
        let auth_date = self.complement - stored_date_9c(input);
        self.expiry_relation
            .holds(today - auth_date, self.expiry_days)
    }

    /// What a run over `authorizations` on the day `today` leaves, as these rules give it:
    /// `MAIN-PARA` [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L136-178] over the
    /// summaries in `ACCNTID` order and the details of each in twin order.
    fn expect(&self, authorizations: &[SummaryInput], today: i32) -> ExpectedPurge {
        let mut owners: Vec<&SummaryInput> = authorizations.iter().collect();
        owners.sort_by_key(|owner| owner.account);
        let mut expected = ExpectedPurge::default();
        for owner in owners {
            expected.counts.summaries_read += 1;
            let mut details = owner.details.to_vec();
            details.sort_by_key(|input| (stored_date_9c(*input), input.time_9c));
            let stored = MemoryCounts {
                approved: owner.approved.into(),
                declined: owner.declined.into(),
            };
            let mut memory = stored;
            let mut expired = Vec::with_capacity(details.len());
            for &input in &details {
                expected.counts.details_read += 1;
                expected.details_before.push(detail_row(owner, input));
                let gone = self.expired(input, today);
                if gone {
                    expected.counts.details_deleted += 1;
                    if resp_code(input) == self.approved_code {
                        memory.approved -= 1;
                    } else {
                        memory.declined -= 1;
                    }
                }
                expired.push(gone);
            }
            let row = summary_row(owner);
            expected.summaries_before.push(row.clone());
            let deleted = self.summary_deleted.holds(memory);
            if deleted {
                expected.counts.summaries_deleted += 1;
                if memory.declined > 0 {
                    expected.e16_accounts.push(owner.account);
                }
            } else {
                if memory != stored {
                    expected.e35_accounts.push(owner.account);
                }
                expected.summaries_after.push(row);
            }
            for (&input, gone) in details.iter().zip(expired) {
                let row = detail_row(owner, input);
                if gone {
                    expected.details_removed.push(row);
                } else if deleted {
                    expected.cascaded.push(row.clone());
                    expected.details_removed.push(row);
                } else {
                    expected.details_after.push(row);
                }
            }
        }
        expected
    }
}

/// The four end-of-job counts `CBPAUP0C` displays
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L173-176].
#[derive(Debug, Default, Clone, Copy)]
struct EndOfJobCounts {
    /// `WS-NO-SUMRY-READ`: every summary `GN` reads.
    summaries_read: u64,
    /// `WS-NO-SUMRY-DELETED`: every summary `DLET`.
    summaries_deleted: u64,
    /// `WS-NO-DTL-READ`: every detail `GNP` reads.
    details_read: u64,
    /// `WS-NO-DTL-DELETED`: every `DLET` of an expired detail, not the details a summary
    /// `DLET` takes with it.
    details_deleted: u64,
}

impl EndOfJobCounts {
    /// The fields of the [`COUNTS_EVENT`], each name with its JSON value token.
    fn fields(self) -> [(&'static str, String); 4] {
        [
            ("summaries_read", self.summaries_read.to_string()),
            ("summaries_deleted", self.summaries_deleted.to_string()),
            ("details_read", self.details_read.to_string()),
            ("details_deleted", self.details_deleted.to_string()),
        ]
    }
}

/// What the audited rules expect of a run over [`AUTHORIZATIONS`] ([`PurgeRules::expect`]).
/// Rows take the forms [`SUMMARIES_SQL`] and [`DETAILS_SQL`] read, in their order.
#[derive(Debug, Default)]
struct ExpectedPurge {
    /// Every summary as committed.
    summaries_before: Vec<Vec<String>>,
    /// Every detail as committed.
    details_before: Vec<Vec<String>>,
    /// The summaries the purge keeps, with their stored counts and amounts (E-35).
    summaries_after: Vec<Vec<String>>,
    /// The details the purge keeps.
    details_after: Vec<Vec<String>>,
    /// The details the purge removes: the expired ones and those their summary takes.
    details_removed: Vec<Vec<String>>,
    /// The four end-of-job counts.
    counts: EndOfJobCounts,
    /// The accounts whose summary goes with a declined count above zero in memory (E-16).
    e16_accounts: Vec<u32>,
    /// The accounts whose summary stays with in-memory counts other than its stored ones
    /// (E-35).
    e35_accounts: Vec<u32>,
    /// The unexpired details that go with their summary.
    cascaded: Vec<Vec<String>>,
}

impl ExpectedPurge {
    /// Checks that the derived outcome still shows what the scenario is built for: a summary
    /// deleted with declines left in memory (E-16), a kept summary whose in-memory counts differ
    /// from its stored ones (E-35), and an unexpired detail deleted with its summary.
    fn check_scenario(&self, rules: &PurgeRules) {
        assert!(
            !self.e16_accounts.is_empty(),
            "no summary goes with declines left in memory, so the run no longer shows E-16: \
             {rules:?}"
        );
        assert!(
            !self.e35_accounts.is_empty(),
            "no kept summary lost counts in memory, so the run no longer shows E-35: {rules:?}"
        );
        assert!(
            !self.cascaded.is_empty(),
            "no unexpired detail goes with its summary, so the run no longer shows the cascade: \
             {rules:?}"
        );
    }
}

/// `text` as a JSON string token. The texts the test compares hold no character JSON escapes.
///
/// # Panics
///
/// Panics if `text` holds a quote, a backslash or a control character.
fn quoted(text: &str) -> String {
    assert!(
        !text
            .chars()
            .any(|ch| ch == '"' || ch == '\\' || ch.is_control()),
        "{text:?} needs JSON escapes"
    );
    format!("\"{text}\"")
}

/// `cents` as [`text_rows`] reads a `NUMERIC(11,2)` column: the units, a point and two digits,
/// by integer arithmetic only.
fn amount_text(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let magnitude = cents.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
}

/// `ACCEPT CURRENT-YYDDD FROM DAY` under the RFC 3339 instant `clock`, derived here from the
/// instant alone: the year's last two digits times 1000 plus the day of the year
/// (P-CBPAUP0C-012).
///
/// # Panics
///
/// Panics if `clock` is no RFC 3339 instant.
fn current_yyddd(clock: &str) -> i32 {
    let day = DateTime::parse_from_rfc3339(clock)
        .unwrap_or_else(|error| panic!("{clock:?} is no RFC 3339 instant: {error}"))
        .naive_utc()
        .date();
    let ordinal = i32::try_from(day.ordinal())
        .unwrap_or_else(|error| panic!("the day of the year of {day}: {error}"));
    day.year() % 100 * 1000 + ordinal
}

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
    let approved_amt = approved_amount_cents(input);
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
        pa_declined_auth_amt: cents(declined_amount_cents(input)),
    }
}

/// The stored `PA-APPROVED-AUTH-AMT` of `input` in cents: 100.00 per approval counted.
fn approved_amount_cents(input: &SummaryInput) -> i64 {
    i64::from(input.approved) * APPROVAL_CENTS
}

/// The stored `PA-DECLINED-AUTH-AMT` of `input` in cents: 40.00 per decline counted.
fn declined_amount_cents(input: &SummaryInput) -> i64 {
    i64::from(input.declined) * DECLINE_CENTS
}

/// The summary `input` as [`SUMMARIES_SQL`] reads it once committed: account, stored counts and
/// stored amounts.
fn summary_row(input: &SummaryInput) -> Vec<String> {
    vec![
        input.account.to_string(),
        input.approved.to_string(),
        input.declined.to_string(),
        amount_text(approved_amount_cents(input)),
        amount_text(declined_amount_cents(input)),
    ]
}

/// `PA-AUTH-DATE-9C` of `input` as `COPAUA0C` stores it: [`AUTH_DATE_COMPLEMENT`] minus the day.
const fn stored_date_9c(input: DetailInput) -> i32 {
    AUTH_DATE_COMPLEMENT - input.yyddd
}

/// `PA-AUTH-RESP-CODE` of `input`: `00` for an approval, `05` for a decline.
const fn resp_code(input: DetailInput) -> &'static str {
    if input.approved { "00" } else { "05" }
}

/// The detail `input` of the summary of `owner` as [`DETAILS_SQL`] reads it once committed:
/// account, stored key and response code.
fn detail_row(owner: &SummaryInput, input: DetailInput) -> Vec<String> {
    vec![
        owner.account.to_string(),
        stored_date_9c(input).to_string(),
        input.time_9c.to_string(),
        resp_code(input).to_owned(),
    ]
}

/// The `PENDING-AUTH-DETAILS` segment `input` describes under the summary of `owner`, keyed by
/// the complemented date `COPAUA0C` stores (P-CBPAUP0C-004). An approval answers `00`/`0000`
/// with match status `P`; a decline answers `05`/`4100` (insufficient funds) with match status
/// `D` and approves 0.00. The request fields are those of the audited CP00 example request.
fn detail(owner: &SummaryInput, input: DetailInput) -> NewPendingAuthDetail {
    let (resp_reason, match_status, approved_cents, transaction_cents) = if input.approved {
        ("0000", MatchStatus::Pending, APPROVAL_CENTS, APPROVAL_CENTS)
    } else {
        ("4100", MatchStatus::AuthDeclined, 0, DECLINE_CENTS)
    };
    NewPendingAuthDetail {
        pa_acct_id: account_id(owner.account),
        pa_auth_date_9c: stored_date_9c(input),
        pa_auth_time_9c: input.time_9c,
        pa_auth_orig_date: Alnum::move_from(input.orig_date),
        pa_auth_orig_time: Alnum::move_from("120000"),
        pa_card_num: CardNumber::move_from(owner.card),
        pa_auth_type: Alnum::move_from("0100"),
        pa_card_expiry_date: Alnum::move_from("1225"),
        pa_message_type: Alnum::move_from("0100"),
        pa_message_source: Alnum::move_from("POS"),
        pa_auth_id_code: Alnum::move_from("120000"),
        pa_auth_resp_code: Alnum::move_from(resp_code(input)),
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

/// A cursor over one JSON text (RFC 8259) that steps over values without decoding them, so a
/// member's value stays its raw token. It stops only at ASCII bytes, which never occur inside a
/// multi-byte UTF-8 character, so every offset it slices at is a character boundary.
struct JsonCursor<'t> {
    /// The text.
    text: &'t str,
    /// The byte offset reached.
    at: usize,
}

impl<'t> JsonCursor<'t> {
    /// The byte at the cursor.
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    /// Steps over JSON whitespace: space, tab, line feed and carriage return.
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// The error of a text that holds something other than `wanted` at the cursor.
    fn unexpected(&self, wanted: &str) -> String {
        format!("{wanted} expected at byte {} of {}", self.at, self.text)
    }

    /// Steps over the string at the cursor, its backslash escapes included, and returns it with
    /// its quotes.
    fn string(&mut self) -> Result<&'t str, String> {
        let start = self.at;
        if self.peek() != Some(b'"') {
            return Err(self.unexpected("a string"));
        }
        self.at += 1;
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(&self.text[start..self.at]);
                }
                // An escape is a backslash and one ASCII character (`\uXXXX` goes on with four
                // more), so an escaped quote never ends the string.
                Some(b'\\') => self.at += 2,
                Some(_) => self.at += 1,
                None => {
                    return Err(format!(
                        "an unterminated string at byte {start} of {}",
                        self.text
                    ));
                }
            }
        }
    }

    /// Steps over the object or array at the cursor, the strings in it included.
    fn nested(&mut self) -> Result<(), String> {
        let start = self.at;
        let mut depth = 0_usize;
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.string()?;
                }
                Some(byte) => {
                    self.at += 1;
                    match byte {
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return Ok(());
                            }
                        }
                        _ => {}
                    }
                }
                None => {
                    return Err(format!(
                        "an unterminated value at byte {start} of {}",
                        self.text
                    ));
                }
            }
        }
    }

    /// Steps over the value at the cursor and returns its raw token: a string with its quotes,
    /// an object or array whole, or a number or literal up to the delimiter after it.
    fn value(&mut self) -> Result<&'t str, String> {
        self.skip_whitespace();
        let start = self.at;
        match self.peek() {
            Some(b'"') => {
                self.string()?;
            }
            Some(b'{' | b'[') => self.nested()?,
            _ => {
                while !matches!(
                    self.peek(),
                    None | Some(b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
                ) {
                    self.at += 1;
                }
            }
        }
        if self.at == start {
            return Err(self.unexpected("a value"));
        }
        Ok(&self.text[start..self.at])
    }

    /// The members of the object at the cursor, in order, each name without its quotes and with
    /// its raw value token.
    fn members(&mut self) -> Result<Vec<(&'t str, &'t str)>, String> {
        self.skip_whitespace();
        if self.peek() != Some(b'{') {
            return Err(self.unexpected("an object"));
        }
        self.at += 1;
        let mut members = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(members);
        }
        loop {
            self.skip_whitespace();
            let name = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(self.unexpected("':'"));
            }
            self.at += 1;
            let value = self.value()?;
            members.push((&name[1..name.len() - 1], value));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(members);
                }
                _ => return Err(self.unexpected("',' or '}'")),
            }
        }
    }
}

/// The members of the JSON object `text`, which holds that object alone, in order, each name
/// with its raw value token.
fn json_members(text: &str) -> Result<Vec<(&str, &str)>, String> {
    let mut cursor = JsonCursor { text, at: 0 };
    let members = cursor.members()?;
    cursor.skip_whitespace();
    if cursor.at == text.len() {
        Ok(members)
    } else {
        Err(cursor.unexpected("the end of the text"))
    }
}

/// One event line of the job's stdout and the members of its `fields` object.
struct Event<'o> {
    /// The line.
    line: &'o str,
    /// The members of `fields`, in order, each name with its raw JSON value token. The members
    /// of the event's span context (`span`, `spans`) are not among them.
    fields: Vec<(&'o str, &'o str)>,
}

/// The members of the `fields` object of the JSON line `line`, or `None` for a line without
/// one, such as a span line.
///
/// # Panics
///
/// Panics, quoting the line, if it is no JSON object, or holds `fields` twice or as anything
/// but an object.
fn event_fields(line: &str) -> Option<Vec<(&str, &str)>> {
    let members = json_members(line)
        .unwrap_or_else(|error| panic!("a stdout line is no JSON object: {error}"));
    let mut fields = members
        .iter()
        .filter(|(name, _)| *name == "fields")
        .map(|(_, value)| *value);
    let object = fields.next()?;
    assert!(fields.next().is_none(), "fields appears twice in {line}");
    Some(
        json_members(object)
            .unwrap_or_else(|error| panic!("the fields of {line} are no JSON object: {error}")),
    )
}

/// The one event on `stdout` whose `message` member is exactly the JSON string `message`.
///
/// # Panics
///
/// Panics, quoting stdout, unless exactly one event carries that message.
fn event<'o>(stdout: &'o str, message: &str) -> Event<'o> {
    let token = quoted(message);
    let mut events = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let fields = event_fields(line)?;
            let messages: Vec<&str> = fields
                .iter()
                .filter(|(name, _)| *name == "message")
                .map(|(_, value)| *value)
                .collect();
            (messages == [token.as_str()]).then_some(Event { line, fields })
        });
    let Some(found) = events.next() else {
        panic!("no {message:?} event on stdout:\n{stdout}");
    };
    assert!(
        events.next().is_none(),
        "more than one {message:?} event on stdout:\n{stdout}"
    );
    found
}

/// Checks that `event` holds each of `expected` exactly once: each a `fields` member name with
/// its exact raw JSON value token, a number unquoted, a boolean bare and a string quoted.
fn assert_fields(event: &Event<'_>, expected: &[(&str, String)]) {
    for (name, token) in expected {
        let found: Vec<&str> = event
            .fields
            .iter()
            .filter(|(member, _)| member == name)
            .map(|(_, value)| *value)
            .collect();
        assert_eq!(
            found,
            [token.as_str()],
            "the {name} field of {}",
            event.line
        );
    }
}

/// Checks the audit rows the test rests on beyond those [`PurgeRules`] reads: every row of
/// [`P_IDS`] exists with an expected result, P-CBPAUP0C-010 and P-CBPAUP0C-011 reproduce E-16
/// and E-35, and P-CBPAUP0C-012 gives `CURRENT-YYDDD` five digits.
fn check_audit_rows(doc: &Document) {
    for id in P_IDS {
        let expected = parity_cell(doc, id, EXPECTED_RESULT);
        assert!(
            !expected.trim().is_empty(),
            "parity row {id} expects nothing"
        );
    }
    for (id, escalation) in [("P-CBPAUP0C-010", "E-16"), ("P-CBPAUP0C-011", "E-35")] {
        let disposition = parity_cell(doc, id, EXPECTED_RESULT);
        assert!(
            disposition.contains(&format!("escalation={escalation}"))
                && disposition.contains("disposition=reproduced"),
            "{id} does not reproduce {escalation}: {disposition}"
        );
    }
    let formats = expected_value(doc, "P-CBPAUP0C-012", "format_regex");
    assert!(
        formats.contains("^[0-9]{5}$"),
        "P-CBPAUP0C-012 gives CURRENT-YYDDD another format: {formats}"
    );
}

#[test]
fn purge_deletes_audited_details_and_summaries() {
    let doc = parity();
    check_audit_rows(&doc);
    let rules = PurgeRules::from_audit(&doc);
    assert_eq!(
        rules.card, SYSIN,
        "the test runs the card P-CBPAUP0C-001 reads"
    );
    let today = current_yyddd(CLOCK);
    assert_eq!(TODAY, today, "CURRENT-YYDDD under {CLOCK}");
    let expected = rules.expect(&AUTHORIZATIONS, today);

    let h = Harness::new();
    block_on(commit_authorizations(&h.db()));
    let summaries_before = text_rows(&h, SUMMARIES_SQL);
    assert_eq!(
        summaries_before, expected.summaries_before,
        "the seeded state holds no summary of its own"
    );
    let details_before = text_rows(&h, DETAILS_SQL);
    assert_eq!(
        details_before, expected.details_before,
        "the seeded state holds no detail of its own"
    );

    let output = h.run_expect(&["cbpaup0j", "--sysin", SYSIN], Some(CLOCK), SUCCESS);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.is_empty(),
        "a normal end writes no stderr, and no {CBPAUP0C_ABENDING:?}: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_fields(&event(&stdout, PARM_EVENT), &rules.parm_fields());
    assert_fields(
        &event(&stdout, TODAY_EVENT),
        &[("today_yyddd", today.to_string())],
    );

    // Read from the test's own pool after the child exited: only committed deletes show
    // (P-CBPAUP0C-008).
    let summaries_after = text_rows(&h, SUMMARIES_SQL);
    assert_eq!(
        summaries_after, expected.summaries_after,
        "the summaries the audited condition keeps (E-16), with their stored counts and \
         amounts (E-35)"
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
        details_after, expected.details_after,
        "only the unexpired details under the kept summaries stay"
    );
    let deleted: Vec<Vec<String>> = details_before
        .into_iter()
        .filter(|row| !details_after.contains(row))
        .collect();
    assert_eq!(
        deleted, expected.details_removed,
        "every expired detail goes, and every detail still under a deleted summary with it"
    );
    assert_fields(&event(&stdout, COUNTS_EVENT), &expected.counts.fields());
    expected.check_scenario(&rules);
}
