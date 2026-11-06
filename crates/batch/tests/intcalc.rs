//! INTCALC (`batch intcalc`): CBACT04C over the transaction category balances after POSTTRAN,
//! against the parity audit's fixture expectations (section D.3), the interest arithmetic they
//! rest on (section C) and the format of the SYSTRAN timestamps (section F).
//!
//! The starting state is the one D.3 derives from: the seeded fixture state after POSTTRAN,
//! which D.1 runs with the clock pinned to [`TRANREPT_CLOCK`]. INTCALC then runs with
//! `PARM='2022071800'` [app/jcl/INTCALC.jcl:L22] under the same clock. The audit pins no
//! INTCALC instant, only the format of its timestamps (P-CBACT04C-176), so they are checked by
//! that format.
//!
//! Amounts are compared as exact `Decimal` values (o3). Checked:
//!
//! - `SYSTRAN(+1)`: one generation of 350-byte `CVTRA05Y` records, each followed by LF, one
//!   per category balance record with a non-zero rate, in key order, each field as the
//!   P-CBACT04C-173 template and its balance and account rows give it.
//! - The accounts: each account but the last in key order gains its total interest and has
//!   its cycle credit and debit set to 0; the last keeps all three (E-14).
//! - The category balances and the live transaction master stay as POSTTRAN left them.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::iter;
use std::str::FromStr;

use carddemo_audit::{CodeSpan, Document, KeyValue, PARITY_COMMON_COLUMNS};
use carddemo_codec::layouts::TransactionLayout;
use carddemo_codec::layouts::transaction::{
    FILLER, TRAN_CARD_NUM, TRAN_CAT_CD, TRAN_DESC, TRAN_ID, TRAN_MERCHANT_CITY, TRAN_MERCHANT_ID,
    TRAN_MERCHANT_NAME, TRAN_MERCHANT_ZIP, TRAN_ORIG_TS, TRAN_PROC_TS, TRAN_SOURCE, TRAN_TYPE_CD,
};
use carddemo_codec::{Charset, FieldSpec, Layout};
use common::{
    Harness, TRANREPT_CLOCK, block_on, expected_span, expected_value, expected_values, parity,
    split_records,
};
use rust_decimal::Decimal;

/// Every audit row this file checks.
///
/// - The starting state: POSTTRAN's counts and exit code over the fixture day.
/// - Section B: the parameter, the account break, the unreachable last update (E-14), the
///   `DEFAULT` fallback, the zero-rate skip without a suffix increment, the computation, the
///   SYSTRAN record fields and the account rewrite.
/// - Section C: no `ROUNDED` anywhere, the truncated quotient, the totals and their receivers,
///   the `DEFAULT` key move and the zero-rate path, all proven by the fixture amounts.
/// - Section D.3: every category balance row, every account row and the SYSTRAN template.
/// - Sections E and F: the last account (E-14), the blank group IDs (E-06), the bytes the
///   COBOL never sets (E-25) and the SYSTRAN timestamp format.
const P_IDS: &[&str] = &[
    // Starting state: POSTTRAN over the fixture day.
    "P-CBTRN02C-065",
    // Section B.
    "P-CBACT04C-001",
    "P-CBACT04C-002",
    "P-CBACT04C-003",
    "P-CBACT04C-004",
    "P-CBACT04C-006",
    "P-CBACT04C-007",
    "P-CBACT04C-008",
    "P-CBACT04C-010",
    // Section C.
    "P-CBACT04C-011",
    "P-CBACT04C-012",
    "P-CBACT04C-020",
    "P-CBACT04C-190",
    "P-CBACT04C-021",
    "P-CBACT04C-191",
    "P-CBACT04C-192",
    "P-CBACT04C-193",
    "P-CBACT04C-022",
    // Section D.3: one row per category balance record, in key order.
    "P-CBACT04C-023",
    "P-CBACT04C-024",
    "P-CBACT04C-025",
    "P-CBACT04C-026",
    "P-CBACT04C-027",
    "P-CBACT04C-028",
    "P-CBACT04C-029",
    "P-CBACT04C-030",
    "P-CBACT04C-031",
    "P-CBACT04C-032",
    "P-CBACT04C-033",
    "P-CBACT04C-034",
    "P-CBACT04C-035",
    "P-CBACT04C-036",
    "P-CBACT04C-037",
    "P-CBACT04C-038",
    "P-CBACT04C-039",
    "P-CBACT04C-040",
    "P-CBACT04C-041",
    "P-CBACT04C-042",
    "P-CBACT04C-043",
    "P-CBACT04C-044",
    "P-CBACT04C-045",
    "P-CBACT04C-046",
    "P-CBACT04C-047",
    "P-CBACT04C-048",
    "P-CBACT04C-049",
    "P-CBACT04C-050",
    "P-CBACT04C-051",
    "P-CBACT04C-052",
    "P-CBACT04C-053",
    "P-CBACT04C-054",
    "P-CBACT04C-055",
    "P-CBACT04C-056",
    "P-CBACT04C-057",
    "P-CBACT04C-058",
    "P-CBACT04C-059",
    "P-CBACT04C-060",
    "P-CBACT04C-061",
    "P-CBACT04C-062",
    "P-CBACT04C-063",
    "P-CBACT04C-064",
    "P-CBACT04C-065",
    "P-CBACT04C-066",
    "P-CBACT04C-067",
    "P-CBACT04C-068",
    "P-CBACT04C-069",
    "P-CBACT04C-070",
    "P-CBACT04C-071",
    "P-CBACT04C-072",
    "P-CBACT04C-073",
    "P-CBACT04C-074",
    "P-CBACT04C-075",
    "P-CBACT04C-076",
    "P-CBACT04C-077",
    "P-CBACT04C-078",
    "P-CBACT04C-079",
    "P-CBACT04C-080",
    "P-CBACT04C-081",
    "P-CBACT04C-082",
    "P-CBACT04C-083",
    "P-CBACT04C-084",
    "P-CBACT04C-085",
    "P-CBACT04C-086",
    "P-CBACT04C-087",
    "P-CBACT04C-088",
    "P-CBACT04C-089",
    "P-CBACT04C-090",
    "P-CBACT04C-091",
    "P-CBACT04C-092",
    "P-CBACT04C-093",
    "P-CBACT04C-094",
    "P-CBACT04C-095",
    "P-CBACT04C-096",
    "P-CBACT04C-097",
    "P-CBACT04C-098",
    "P-CBACT04C-099",
    "P-CBACT04C-100",
    "P-CBACT04C-101",
    "P-CBACT04C-102",
    "P-CBACT04C-103",
    "P-CBACT04C-104",
    "P-CBACT04C-105",
    "P-CBACT04C-106",
    "P-CBACT04C-107",
    "P-CBACT04C-108",
    "P-CBACT04C-109",
    "P-CBACT04C-110",
    "P-CBACT04C-111",
    "P-CBACT04C-112",
    "P-CBACT04C-113",
    "P-CBACT04C-114",
    "P-CBACT04C-115",
    "P-CBACT04C-116",
    "P-CBACT04C-117",
    "P-CBACT04C-118",
    "P-CBACT04C-119",
    "P-CBACT04C-120",
    "P-CBACT04C-121",
    "P-CBACT04C-122",
    // Section D.3: one row per account.
    "P-CBACT04C-123",
    "P-CBACT04C-124",
    "P-CBACT04C-125",
    "P-CBACT04C-126",
    "P-CBACT04C-127",
    "P-CBACT04C-128",
    "P-CBACT04C-129",
    "P-CBACT04C-130",
    "P-CBACT04C-131",
    "P-CBACT04C-132",
    "P-CBACT04C-133",
    "P-CBACT04C-134",
    "P-CBACT04C-135",
    "P-CBACT04C-136",
    "P-CBACT04C-137",
    "P-CBACT04C-138",
    "P-CBACT04C-139",
    "P-CBACT04C-140",
    "P-CBACT04C-141",
    "P-CBACT04C-142",
    "P-CBACT04C-143",
    "P-CBACT04C-144",
    "P-CBACT04C-145",
    "P-CBACT04C-146",
    "P-CBACT04C-147",
    "P-CBACT04C-148",
    "P-CBACT04C-149",
    "P-CBACT04C-150",
    "P-CBACT04C-151",
    "P-CBACT04C-152",
    "P-CBACT04C-153",
    "P-CBACT04C-154",
    "P-CBACT04C-155",
    "P-CBACT04C-156",
    "P-CBACT04C-157",
    "P-CBACT04C-158",
    "P-CBACT04C-159",
    "P-CBACT04C-160",
    "P-CBACT04C-161",
    "P-CBACT04C-162",
    "P-CBACT04C-163",
    "P-CBACT04C-164",
    "P-CBACT04C-165",
    "P-CBACT04C-166",
    "P-CBACT04C-167",
    "P-CBACT04C-168",
    "P-CBACT04C-169",
    "P-CBACT04C-170",
    "P-CBACT04C-171",
    "P-CBACT04C-172",
    // Section D.3: the SYSTRAN record template.
    "P-CBACT04C-173",
    // Sections E and F.
    "P-CBACT04C-174",
    "P-CVACT01Y-019",
    "P-CVTRA05Y-018",
    "P-CBACT04C-176",
];

/// The exit code of a run that reaches the end of the category balance file (AAP 0.5.3.5).
const SUCCESS: i32 = 0;

/// The INTCALC command of the JCL: `PARM='2022071800'` [app/jcl/INTCALC.jcl:L22].
const INTCALC: [&str; 3] = ["intcalc", "--parm-date", PARM_DATE];

/// `PARM-DATE X(10)`, the JCL `PARM` (P-CBACT04C-001).
const PARM_DATE: &str = "2022071800";

/// The generation dataset INTCALC writes, `SYSTRAN(+1)` [app/jcl/INTCALC.jcl:L37-41].
const SYSTRAN: &str = "SYSTRAN";

/// The file of the first generation of `SYSTRAN` (AAP 0.5.3.5).
const FIRST_GENERATION: &str = "SYSTRAN.G0001V00";

/// The `###` heading of the INTCALC fixture expectations.
const INTCALC_SUBSECTION: &str = "D.3 INTCALC";

/// The audit row of POSTTRAN's counts and exit code over the fixture day.
const POSTTRAN_COUNTS: &str = "P-CBTRN02C-065";

/// The audit row of the SYSTRAN record template.
const SYSTRAN_TEMPLATE: &str = "P-CBACT04C-173";

/// The audit row of the SYSTRAN timestamp format.
const SYSTRAN_TIMESTAMP: &str = "P-CBACT04C-176";

/// The template key of the FILLER bytes.
const FILLER_KEY: &str = "filler";

/// The fields the template sets to spaces, by their template key.
const BLANK_FIELDS: [(&str, FieldSpec); 3] = [
    ("tran_merchant_name", TRAN_MERCHANT_NAME),
    ("tran_merchant_city", TRAN_MERCHANT_CITY),
    ("tran_merchant_zip", TRAN_MERCHANT_ZIP),
];

/// Every account's `ACCT-CURR-BAL`, `ACCT-CURR-CYC-CREDIT` and `ACCT-CURR-CYC-DEBIT`, in key
/// order; columns as `migrations/0002_master_data.sql` defines them.
const ACCOUNTS_SQL: &str = "SELECT account_id, acct_curr_bal, acct_curr_cyc_credit, \
     acct_curr_cyc_debit FROM accounts ORDER BY account_id";

/// Every category balance record in the `TCATBALF` key order CBACT04C reads it in, the order
/// of the padded-key index of `migrations/0003_transactions.sql`.
const TCATBAL_SQL: &str = "SELECT trancat_acct_id, trancat_type_cd, trancat_cd, tran_cat_bal \
     FROM transaction_category_balances \
     ORDER BY trancat_acct_id, rpad(trancat_type_cd, 2) COLLATE \"C\", trancat_cd";

/// The `Expected result` column of every parity audit data table.
const EXPECTED_RESULT: &str = PARITY_COMMON_COLUMNS[5];

/// The balance and cycle amounts of every account, by account number.
type AccountAmounts = BTreeMap<u64, [Decimal; 3]>;

/// One category balance record: account, type, category and balance.
type CategoryBalance = (u64, String, u16, Decimal);

/// The interest record of one category balance record.
#[derive(Debug)]
struct Interest {
    /// `TRAN-ID` of the SYSTRAN record.
    tran_id: String,
    /// `WS-MONTHLY-INT`, the record's `TRAN-AMT`.
    monthly: Decimal,
}

/// A D.3 category balance row (P-CBACT04C-023 to P-CBACT04C-122).
#[derive(Debug)]
struct BalanceRow {
    /// The row ID.
    id: String,
    /// `TRANCAT-ACCT-ID 9(11)` as its 11 digits.
    account_id: String,
    /// `TRANCAT-TYPE-CD X(02)`.
    type_cd: String,
    /// `TRANCAT-CD 9(04)`.
    cat_cd: u16,
    /// `TRAN-CAT-BAL` after POSTTRAN.
    tran_cat_bal: Decimal,
    /// The interest record, or `None` for a zero rate, which writes none.
    interest: Option<Interest>,
}

/// A D.3 account row (P-CBACT04C-123 to P-CBACT04C-172).
#[derive(Debug)]
struct AccountRow {
    /// The row ID.
    id: String,
    /// `ACCT-ID 9(11)` as its 11 digits.
    account_id: String,
    /// `WS-TOTAL-INT` of the account.
    total_interest: Decimal,
    /// Whether `1050-UPDATE-ACCOUNT` rewrites the account.
    updated: bool,
    /// `ACCT-CURR-BAL`, `ACCT-CURR-CYC-CREDIT` and `ACCT-CURR-CYC-DEBIT` after INTCALC.
    amounts: [Decimal; 3],
    /// The `TRAN-ID` of the account's SYSTRAN record.
    systran_tran_id: String,
    /// `XREF-CARD-NUM`, the record's `TRAN-CARD-NUM`.
    card_number: String,
}

/// The INTCALC expectations of audit section D.3.
struct Expectations {
    /// The category balance rows, in key order.
    balances: Vec<BalanceRow>,
    /// The account rows, by `ACCT-ID`.
    accounts: BTreeMap<String, AccountRow>,
}

/// The `key=value` pairs of one audit row's `Expected result` cell.
struct Pairs<'a> {
    /// The row ID, for messages.
    id: &'a str,
    /// The pairs, in order.
    pairs: &'a [KeyValue],
}

impl<'a> Pairs<'a> {
    /// The value of the first pair whose key names `key`; a key such as
    /// `tran_merchant_name, tran_merchant_city` names each of its comma-separated parts.
    fn get(&self, key: &str) -> &'a str {
        self.pairs
            .iter()
            .find(|pair| pair.key.split(',').any(|part| part.trim() == key))
            .map_or_else(
                || panic!("parity audit row {} expects no {key}=", self.id),
                |pair| pair.value.as_str(),
            )
    }

    /// Whether a pair names `key`.
    fn has(&self, key: &str) -> bool {
        self.pairs.iter().any(|pair| pair.key == key)
    }

    /// The value of `key` as a decimal amount or rate.
    fn decimal(&self, key: &str) -> Decimal {
        decimal(self.get(key), &format!("{} {key}", self.id))
    }
}

impl Expectations {
    /// Reads every data row of the D.3 subsection except the template, and checks that the
    /// rows fit together as CBACT04C's loop requires: every balance row's account has an
    /// account row and every account row has balance rows, each account's total interest is
    /// the sum of its monthly interest (P-CBACT04C-020), the interest rows number the
    /// template's `records`, and the only account not updated is the last in key order
    /// (E-14).
    ///
    /// # Panics
    ///
    /// On a row of another shape or a value that does not parse, naming the row; on rows that
    /// do not fit together.
    fn from_audit(doc: &Document) -> Self {
        let mut balances = Vec::new();
        let mut accounts = BTreeMap::new();
        for (id, values) in intcalc_rows(doc) {
            let pairs = Pairs {
                id: &id,
                pairs: &values,
            };
            if pairs.has("kind") {
                assert_eq!(pairs.get("kind"), "systran_template", "{id}");
            } else if pairs.has("rate") {
                balances.push(balance_row(&pairs));
            } else if pairs.has("total_interest") {
                let row = account_row(&pairs);
                let duplicate = accounts.insert(row.account_id.clone(), row);
                assert!(duplicate.is_none(), "{id}: a second row for its account");
            } else {
                panic!("{id}: neither a balance, an account nor the template row");
            }
        }
        let expectations = Self { balances, accounts };
        expectations.check_consistency(doc);
        expectations
    }

    /// The consistency checks of [`Expectations::from_audit`].
    fn check_consistency(&self, doc: &Document) {
        let mut totals: BTreeMap<&str, Decimal> = BTreeMap::new();
        for row in &self.balances {
            let total = totals.entry(row.account_id.as_str()).or_default();
            *total += row.interest.as_ref().map_or(Decimal::ZERO, |i| i.monthly);
        }
        let audited: BTreeSet<&str> = self.accounts.keys().map(String::as_str).collect();
        assert_eq!(
            totals.keys().copied().collect::<BTreeSet<_>>(),
            audited,
            "the balance rows and the account rows name different accounts"
        );
        for (account_id, total) in &totals {
            let row = self.account(account_id);
            assert_eq!(row.total_interest, *total, "{}: total_interest", row.id);
        }
        let records: usize = expected_value(doc, SYSTRAN_TEMPLATE, "records")
            .parse()
            .expect("P-CBACT04C-173 gives a record count");
        assert_eq!(self.interest_rows().count(), records, "interest rows");
        let not_updated: Vec<&str> = self
            .accounts
            .values()
            .filter(|row| !row.updated)
            .map(|row| row.account_id.as_str())
            .collect();
        let last = self.balances.last().map(|row| row.account_id.as_str());
        assert_eq!(
            not_updated,
            last.into_iter().collect::<Vec<_>>(),
            "E-14: only the last account in key order is not updated"
        );
    }

    /// The balance rows with an interest record, in key order: the order of `SYSTRAN(+1)`.
    fn interest_rows(&self) -> impl Iterator<Item = (&BalanceRow, &Interest)> {
        self.balances
            .iter()
            .filter_map(|row| row.interest.as_ref().map(|interest| (row, interest)))
    }

    /// The account row of `account_id`.
    fn account(&self, account_id: &str) -> &AccountRow {
        self.accounts
            .get(account_id)
            .unwrap_or_else(|| panic!("section D.3 has no account row for {account_id}"))
    }

    /// The category balance records after POSTTRAN, in key order.
    fn category_balances(&self) -> Vec<CategoryBalance> {
        self.balances
            .iter()
            .map(|row| {
                (
                    number(&row.account_id, &row.id),
                    row.type_cd.clone(),
                    row.cat_cd,
                    row.tran_cat_bal,
                )
            })
            .collect()
    }
}

/// Every data row of the D.3 subsection, in order, with the `key=value` pairs of its
/// `Expected result` cell.
fn intcalc_rows(doc: &Document) -> Vec<(String, Vec<KeyValue>)> {
    doc.tables()
        .iter()
        .filter(|table| {
            table.is_data_table() && table.subsection.as_deref() == Some(INTCALC_SUBSECTION)
        })
        .flat_map(|table| {
            table.rows.iter().map(move |row| {
                let id = table.cell(row, "ID").unwrap_or_default().to_owned();
                let expected = table
                    .cell(row, EXPECTED_RESULT)
                    .unwrap_or_else(|| panic!("{id} has no {EXPECTED_RESULT:?} cell"));
                (id, carddemo_audit::key_values(expected))
            })
        })
        .collect()
}

/// A D.3 category balance row. A monthly interest of `none` goes with a zero rate, and
/// only a record with interest has a `tran_id` (P-CBACT04C-006, P-CBACT04C-022).
fn balance_row(pairs: &Pairs<'_>) -> BalanceRow {
    let rate = pairs.decimal("rate");
    let monthly = pairs.get("monthly_interest");
    let interest = if monthly.starts_with("none") {
        assert!(!pairs.has("tran_id"), "{}: no record, no tran_id", pairs.id);
        None
    } else {
        Some(Interest {
            tran_id: pairs.get("tran_id").to_owned(),
            monthly: pairs.decimal("monthly_interest"),
        })
    };
    assert_eq!(
        interest.is_none(),
        rate.is_zero(),
        "{}: an interest record exactly when the rate is not 0",
        pairs.id
    );
    assert_eq!(pairs.get("group"), "DEFAULT", "{}: E-06", pairs.id);
    BalanceRow {
        id: pairs.id.to_owned(),
        account_id: pairs.get("account_id").to_owned(),
        type_cd: pairs.get("tran_type_cd").to_owned(),
        cat_cd: pairs.get("tran_cat_cd").parse().unwrap_or_else(|error| {
            panic!("{}: tran_cat_cd is not a 9(04) value: {error}", pairs.id)
        }),
        tran_cat_bal: pairs.decimal("tran_cat_bal"),
        interest,
    }
}

/// A D.3 account row: `updated=yes`, or `updated=no` with its reason.
fn account_row(pairs: &Pairs<'_>) -> AccountRow {
    let updated = pairs.get("updated");
    AccountRow {
        id: pairs.id.to_owned(),
        account_id: pairs.get("account_id").to_owned(),
        total_interest: pairs.decimal("total_interest"),
        updated: match updated.split_whitespace().next() {
            Some("yes") => true,
            Some("no") => false,
            _ => panic!("{}: updated={updated} is neither yes nor no", pairs.id),
        },
        amounts: [
            pairs.decimal("acct_curr_bal"),
            pairs.decimal("acct_curr_cyc_credit"),
            pairs.decimal("acct_curr_cyc_debit"),
        ],
        systran_tran_id: pairs.get("systran_tran_id").to_owned(),
        card_number: pairs.get("card_number").to_owned(),
    }
}

/// The SYSTRAN record template of P-CBACT04C-173 and the timestamp format of P-CBACT04C-176,
/// each field as the stored bytes the replacement writes.
struct SystranTemplate {
    /// `TRAN-ID`: `{PARM-DATE}{WS-TRANID-SUFFIX}`.
    tran_id: String,
    /// `TRAN-TYPE-CD`.
    type_cd: String,
    /// `TRAN-CAT-CD 9(04)`: the literal `MOVE`d, right-justified and zero-filled, as an
    /// alphanumeric literal moved to an unsigned numeric item is stored (P-CBACT04C-008:
    /// `'05'` is stored `0005`).
    cat_cd: String,
    /// `TRAN-SOURCE`, padded to its width.
    source: String,
    /// `TRAN-DESC`, its `{ACCT-ID}` still to fill, and the width it is padded to.
    desc: (String, Option<usize>),
    /// `TRAN-MERCHANT-ID`.
    merchant_id: String,
    /// The FILLER bytes: `{UNDEF:20}`.
    filler: String,
    /// The pattern `TRAN-ORIG-TS` and `TRAN-PROC-TS` match.
    timestamp_format: String,
}

impl SystranTemplate {
    /// Reads the template.
    ///
    /// # Panics
    ///
    /// When a field the template must give is missing or not an exact text where one is
    /// expected, or when it sets the merchant name, city or zip to anything but spaces.
    fn from_audit(doc: &Document) -> Self {
        let id = SYSTRAN_TEMPLATE;
        let values = expected_values(doc, id);
        let pairs = Pairs { id, pairs: &values };
        for (key, _) in BLANK_FIELDS {
            assert_eq!(pairs.get(key), "spaces", "{id}: {key}");
        }
        let cat_literal = pairs.get("tran_cat_cd");
        assert!(
            cat_literal.bytes().all(|byte| byte.is_ascii_digit()),
            "{id}: tran_cat_cd={cat_literal} is no unsigned numeric literal"
        );
        let source = expected_span(doc, id, "tran_source");
        let desc = expected_span(doc, id, "tran_desc");
        let timestamp = expected_span(doc, SYSTRAN_TIMESTAMP, "format_regex");
        Self {
            tran_id: pairs.get("tran_id").to_owned(),
            type_cd: pairs.get("tran_type_cd").to_owned(),
            cat_cd: format!("{cat_literal:0>width$}", width = TRAN_CAT_CD.len),
            source: expand(&unquote(&source, id), &[], source.width),
            desc: (unquote(&desc, id), desc.width),
            merchant_id: pairs.get("tran_merchant_id").to_owned(),
            filler: expand(pairs.get(FILLER_KEY), &[], None),
            timestamp_format: unquote(&timestamp, SYSTRAN_TIMESTAMP),
        }
    }

    /// Checks the SYSTRAN record `record`, the `suffix`-th written, against the template, the
    /// category balance row it was computed from and the row of that row's account.
    fn check(&self, record: &str, suffix: usize, balance: &BalanceRow, account: &AccountRow) {
        let at = &balance.id;
        let interest = balance
            .interest
            .as_ref()
            .unwrap_or_else(|| panic!("{at}: a zero-rate balance has no record"));
        let suffix = format!("{suffix:06}");
        let tran_id = expand(
            &self.tran_id,
            &[("PARM-DATE", PARM_DATE), ("WS-TRANID-SUFFIX", &suffix)],
            None,
        );
        let desc = expand(
            &self.desc.0,
            &[("ACCT-ID", &balance.account_id)],
            self.desc.1,
        );
        let expected = [
            (TRAN_ID, tran_id.as_str()),
            (TRAN_ID, interest.tran_id.as_str()),
            (TRAN_TYPE_CD, self.type_cd.as_str()),
            (TRAN_CAT_CD, self.cat_cd.as_str()),
            (TRAN_SOURCE, self.source.as_str()),
            (TRAN_DESC, desc.as_str()),
            (TRAN_MERCHANT_ID, self.merchant_id.as_str()),
            (TRAN_CARD_NUM, account.card_number.as_str()),
            (FILLER, self.filler.as_str()),
        ];
        for (spec, value) in expected {
            assert_eq!(field(record, &spec), value, "{at}: {}", spec.name);
        }
        for (_, spec) in BLANK_FIELDS {
            let spaces = " ".repeat(spec.len);
            assert_eq!(field(record, &spec), spaces, "{at}: {}", spec.name);
        }
        let decoded = TransactionLayout::decode(record.as_bytes(), Charset::Ascii)
            .unwrap_or_else(|error| panic!("{at}: SYSTRAN record does not decode: {error}"));
        assert_eq!(
            decoded.transaction.tran_amt.to_decimal(),
            interest.monthly,
            "{at}: TRAN-AMT is the monthly interest"
        );
        let orig_ts = field(record, &TRAN_ORIG_TS);
        assert_eq!(field(record, &TRAN_PROC_TS), orig_ts, "{at}: one timestamp");
        assert!(
            matches_format(&self.timestamp_format, orig_ts),
            "{at}: TRAN-ORIG-TS {orig_ts:?} is not in the {SYSTRAN_TIMESTAMP} format"
        );
    }
}

/// The records of the one `SYSTRAN` generation INTCALC catalogs, `SYSTRAN.G0001V00`: 350-byte
/// `CVTRA05Y` records, each followed by LF.
fn systran_records(h: &Harness) -> Vec<String> {
    let generations = h.generations(SYSTRAN);
    let names: Vec<_> = generations
        .iter()
        .filter_map(|path| path.file_name()?.to_str())
        .collect();
    assert_eq!(
        names,
        [FIRST_GENERATION],
        "the cataloged SYSTRAN generations"
    );
    split_records(&h.newest(SYSTRAN), TransactionLayout::LEN)
}

/// Checks `SYSTRAN(+1)` record for record against the interest rows of section D.3 in key
/// order (P-CBACT04C-008, P-CBACT04C-173), and returns the TRAN-IDs and amounts written for
/// each account, by `ACCT-ID`.
///
/// The suffix runs from 1 in write order, so the zero-rate records in between, which write
/// nothing, take no suffix (P-CBACT04C-006).
fn check_systran(
    doc: &Document,
    expected: &Expectations,
    records: &[String],
) -> BTreeMap<String, Vec<(String, Decimal)>> {
    let template = SystranTemplate::from_audit(doc);
    let rows: Vec<_> = expected.interest_rows().collect();
    assert_eq!(
        records.len(),
        rows.len(),
        "one SYSTRAN record per category balance with a non-zero rate"
    );
    for (key, record) in [("first", records.first()), ("last", records.last())] {
        let tran_id = record.map(|record| field(record, &TRAN_ID));
        assert_eq!(
            tran_id,
            Some(expected_value(doc, SYSTRAN_TEMPLATE, key).as_str()),
            "{SYSTRAN_TEMPLATE}: {key}"
        );
    }
    let mut written: BTreeMap<String, Vec<(String, Decimal)>> = BTreeMap::new();
    for (index, (record, (balance, interest))) in records.iter().zip(rows).enumerate() {
        let account = expected.account(&balance.account_id);
        template.check(record, index + 1, balance, account);
        written
            .entry(balance.account_id.clone())
            .or_default()
            .push((field(record, &TRAN_ID).to_owned(), interest.monthly));
    }
    written
}

/// Checks every account after INTCALC against its D.3 row and its state `before` the run
/// (P-CBACT04C-010, P-CBACT04C-021, P-CBACT04C-191, P-CBACT04C-192): an updated account gains
/// the total of its SYSTRAN amounts, which is its audited total interest, and has both cycle
/// amounts set to 0; the last account keeps all three (P-CBACT04C-003, P-CBACT04C-174).
fn check_accounts(
    expected: &Expectations,
    before: &AccountAmounts,
    after: &AccountAmounts,
    written: &BTreeMap<String, Vec<(String, Decimal)>>,
) {
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        "INTCALC adds and removes no account"
    );
    assert_eq!(
        after.len(),
        expected.accounts.len(),
        "every account is audited"
    );
    for row in expected.accounts.values() {
        let at = &row.id;
        let key = number(&row.account_id, at);
        let (was, now) = (amounts_of(before, key, at), amounts_of(after, key, at));
        let records = written.get(&row.account_id).map_or(&[][..], Vec::as_slice);
        let tran_ids: Vec<&str> = records.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            tran_ids,
            [row.systran_tran_id.as_str()],
            "{at}: SYSTRAN records"
        );
        let total: Decimal = records.iter().map(|(_, amount)| *amount).sum();
        assert_eq!(total, row.total_interest, "{at}: WS-TOTAL-INT");
        assert_eq!(
            now, row.amounts,
            "{at}: balance, cycle credit and cycle debit"
        );
        if row.updated {
            assert_eq!(
                now[0],
                was[0] + total,
                "{at}: ADD WS-TOTAL-INT TO ACCT-CURR-BAL"
            );
            assert_eq!(now[1..], [Decimal::ZERO; 2], "{at}: cycle amounts set to 0");
        } else {
            assert_eq!(now, was, "{at}: the last account is not updated (E-14)");
        }
    }
}

/// The amounts of account `key` in `amounts`.
fn amounts_of(amounts: &AccountAmounts, key: u64, at: &str) -> [Decimal; 3] {
    *amounts
        .get(&key)
        .unwrap_or_else(|| panic!("{at}: account {key} is not in the database"))
}

/// The balance and cycle amounts of every account in the database.
fn account_amounts(h: &Harness) -> AccountAmounts {
    block_on(h.stack().fetch_text_rows(ACCOUNTS_SQL))
        .into_iter()
        .map(|row| {
            let [id, bal, credit, debit] = columns::<4>(row, "accounts");
            let at = format!("account {id}");
            (
                number(&id, &at),
                [
                    decimal(&bal, &at),
                    decimal(&credit, &at),
                    decimal(&debit, &at),
                ],
            )
        })
        .collect()
}

/// Every category balance record in the database, in `TCATBALF` key order.
fn category_balances(h: &Harness) -> Vec<CategoryBalance> {
    block_on(h.stack().fetch_text_rows(TCATBAL_SQL))
        .into_iter()
        .map(|row| {
            let [account, type_cd, cat_cd, bal] =
                columns::<4>(row, "transaction_category_balances");
            let at = format!("category balance {account}/{type_cd}/{cat_cd}");
            let cat = cat_cd
                .parse()
                .unwrap_or_else(|error| panic!("{at}: trancat_cd: {error}"));
            (number(&account, &at), type_cd, cat, decimal(&bal, &at))
        })
        .collect()
}

/// The `N` non-null text columns of a database row of `table`.
fn columns<const N: usize>(row: Vec<Option<String>>, table: &str) -> [String; N] {
    let values: Vec<String> = row
        .into_iter()
        .map(|value| value.unwrap_or_else(|| panic!("{table}: a NOT NULL column is null")))
        .collect();
    values
        .try_into()
        .unwrap_or_else(|values: Vec<String>| panic!("{table}: {} columns, not {N}", values.len()))
}

/// The bytes of field `spec` of the record `record`.
fn field<'r>(record: &'r str, spec: &FieldSpec) -> &'r str {
    record
        .get(spec.offset..spec.end())
        .unwrap_or_else(|| panic!("{} lies outside the record", spec.name))
}

/// `text` as a decimal amount or rate.
fn decimal(text: &str, at: &str) -> Decimal {
    Decimal::from_str(text).unwrap_or_else(|error| panic!("{at}: {text:?} is no decimal: {error}"))
}

/// `text`, an unsigned `9(n)` value such as an `ACCT-ID`, as a number.
fn number(text: &str, at: &str) -> u64 {
    text.parse()
        .unwrap_or_else(|error| panic!("{at}: {text:?} is no unsigned number: {error}"))
}

/// The content of the exact text `span` of row `id`: the characters between its double
/// quotes, with the escapes `\"` and `\\` read.
fn unquote(span: &CodeSpan, id: &str) -> String {
    let inner = span
        .text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id}: {:?} is no double-quoted exact text", span.text));
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some(escaped @ ('"' | '\\')) => out.push(escaped),
                other => panic!("{id}: {other:?} after a backslash in {:?}", span.text),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// The text the replacement writes for the audited text `text`: each `{FIELD}` replaced by
/// its value in `fields`, each `{UNDEF:n}` by n spaces, then right-padded with spaces to
/// `width`.
///
/// `{UNDEF:n}` marks bytes the COBOL never sets. They are excluded from parity, and the
/// replacement writes spaces in them (E-25, P-CVTRA05Y-018), which is what is checked.
fn expand(text: &str, fields: &[(&str, &str)], width: Option<usize>) -> String {
    let mut out = String::with_capacity(width.unwrap_or(text.len()));
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = open
            + rest[open..]
                .find('}')
                .unwrap_or_else(|| panic!("an unclosed {{ in {text:?}"));
        let name = &rest[open + 1..close];
        if let Some(count) = name.strip_prefix("UNDEF:") {
            let count: usize = count
                .parse()
                .unwrap_or_else(|error| panic!("{{{name}}} in {text:?}: {error}"));
            out.extend(iter::repeat_n(' ', count));
        } else {
            let value = fields.iter().find(|(field, _)| *field == name).map_or_else(
                || panic!("no value for {{{name}}} in {text:?}"),
                |pair| pair.1,
            );
            out.push_str(value);
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    if let Some(width) = width {
        assert!(
            out.len() <= width,
            "{out:?} is longer than its width {width}"
        );
        out.extend(iter::repeat_n(' ', width - out.len()));
    }
    out
}

/// One position of a fixed-width format: a literal character or a character range.
#[derive(Debug, Clone, Copy)]
enum Atom {
    /// Exactly this character.
    Literal(char),
    /// Any character from the first to the second, both included.
    Range(char, char),
}

impl Atom {
    fn accepts(self, ch: char) -> bool {
        match self {
            Self::Literal(literal) => ch == literal,
            Self::Range(low, high) => (low..=high).contains(&ch),
        }
    }
}

/// Whether `text` matches the audited format `pattern` in full.
///
/// The audit's fixed-width formats use a subset of regular-expression syntax, read here:
/// `^` and `$` around the whole pattern, literal characters, `\` escapes, one-range classes
/// such as `[0-9]`, and a count `{n}` after any of them.
///
/// # Panics
///
/// On any other syntax, so a format outside the subset fails the test instead of matching
/// loosely.
fn matches_format(pattern: &str, text: &str) -> bool {
    let body = pattern
        .strip_prefix('^')
        .and_then(|body| body.strip_suffix('$'))
        .unwrap_or_else(|| panic!("format {pattern:?} is not anchored at both ends"));
    let mut atoms = Vec::new();
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        let atom = match ch {
            '\\' => Atom::Literal(
                chars
                    .next()
                    .unwrap_or_else(|| panic!("{pattern:?}: \\ at end")),
            ),
            '[' => {
                let class: String = chars.by_ref().take(4).collect();
                let mut parts = class.chars();
                match (parts.next(), parts.next(), parts.next(), parts.next()) {
                    (Some(low), Some('-'), Some(high), Some(']')) => Atom::Range(low, high),
                    _ => panic!("{pattern:?}: class [{class} is not a single range"),
                }
            }
            '(' | ')' | '|' | '*' | '+' | '?' | '.' | '{' | '}' | ']' | '^' | '$' => {
                panic!("{pattern:?}: {ch:?} is outside the fixed-width subset")
            }
            literal => Atom::Literal(literal),
        };
        let count = if chars.next_if_eq(&'{').is_some() {
            let digits: String = chars.by_ref().take_while(|&digit| digit != '}').collect();
            digits
                .parse()
                .unwrap_or_else(|error| panic!("{pattern:?}: count {{{digits}}}: {error}"))
        } else {
            1
        };
        atoms.extend(iter::repeat_n(atom, count));
    }
    let text: Vec<char> = text.chars().collect();
    text.len() == atoms.len() && atoms.iter().zip(text).all(|(atom, ch)| atom.accepts(ch))
}

#[test]
fn p_ids_name_every_intcalc_row_and_only_audit_rows() {
    let doc = parity();
    let listed: BTreeSet<&str> = P_IDS.iter().copied().collect();
    assert_eq!(listed.len(), P_IDS.len(), "P_IDS lists an ID twice");
    for (id, _) in intcalc_rows(&doc) {
        assert!(
            listed.contains(id.as_str()),
            "{id} of section D.3 is not in P_IDS"
        );
    }
    for id in P_IDS {
        assert!(doc.find_row(id).is_some(), "{id} is no parity audit row");
    }
    // The rows parse and fit together, and the template and timestamp format read.
    let expected = Expectations::from_audit(&doc);
    let template = SystranTemplate::from_audit(&doc);
    assert!(!expected.balances.is_empty() && !expected.accounts.is_empty());
    // The format reads as the X(26) Db2 timestamp: TRANREPT_CLOCK in that form matches, the
    // ISO form of the same instant does not.
    for (timestamp, matches) in [
        ("2022-07-01-12.00.00.000000", true),
        ("2022-07-01 12:00:00.000000", false),
        ("2022-07-01-12.00.00.00000", false),
    ] {
        assert_eq!(
            matches_format(&template.timestamp_format, timestamp),
            matches,
            "{SYSTRAN_TIMESTAMP}: {timestamp:?}"
        );
    }
}

#[test]
fn intcalc_after_posttran_writes_the_audited_interest_and_account_updates() {
    let doc = parity();
    let expected = Expectations::from_audit(&doc);
    let h = Harness::new();
    let posttran_exit: i32 = expected_value(&doc, POSTTRAN_COUNTS, "exit")
        .parse()
        .expect("P-CBTRN02C-065 gives a numeric exit code");
    // D.3 starts after POSTTRAN, which D.1 runs with the clock pinned to TRANREPT_CLOCK;
    // INTCALC runs under the same clock, so the whole cycle sees one processing instant.
    let _ = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), posttran_exit);
    let accounts_before = account_amounts(&h);
    let balances_before = category_balances(&h);
    let master_before = h.live_master_rows();
    assert_eq!(
        balances_before,
        expected.category_balances(),
        "the category balances after POSTTRAN, in key order, are the audited INTCALC input"
    );

    let _ = h.run_expect(&INTCALC, Some(TRANREPT_CLOCK), SUCCESS);

    let written = check_systran(&doc, &expected, &systran_records(&h));
    check_accounts(&expected, &accounts_before, &account_amounts(&h), &written);
    assert_eq!(
        category_balances(&h),
        balances_before,
        "CBACT04C opens TCATBALF for input only"
    );
    assert_eq!(
        h.live_master_rows(),
        master_before,
        "the SYSTRAN records do not enter the live transaction master"
    );
}
