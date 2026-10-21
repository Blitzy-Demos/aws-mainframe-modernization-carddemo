//! Codec A over the whole fixture corpus and over every record layout (binding i8, AAP 0.9.1 and
//! 0.9.2 outcome o4; the `FIXFRM`, `FIXUSR` and `FIXLOAD` rows of `modern/audit/fixtures.md` and
//! the field rows of `modern/audit/copybooks.md`).
//!
//! - Byte identity: each of the 626 lines of the nine files of `app/data/ASCII/`, and each of the
//!   ten in-stream USRSEC records of `app/jcl/DUSRSECJ.jcl:35-44`, decodes through
//!   `domain::batch::loads` into its typed record and re-encodes to the identical bytes, its
//!   terminator and any short body included. Lines are split here with `split_inclusive`, apart
//!   from the codec's own splitter, so the comparison does not trust the code under test.
//! - Typed values: dates and timestamps decode to typed values, never text (Typed Dates and
//!   Timestamps, E33), and one decoded value per file is compared with the fixture text.
//! - Properties: for every `FixedRecord` layout in `domain::records`, `decode(encode(r)) == r`
//!   over generated records; for the ten layouts read from fixture lines, also the `Framed`
//!   envelope with every terminator and every accepted short body.
//! - Snapshot: one insta snapshot fixes the decoded corpus. Card numbers, SSNs, CVVs and passwords
//!   appear in it only in their redacted `Debug` forms.
//!
//! Fixture bytes hold card numbers, SSNs and passwords, so no assertion here compares or prints a
//! raw byte buffer with `assert_eq!`: a failure names the file, the line and the field only
//! (Cardholder Data Redaction). The fixtures are read in place; the test needs no container,
//! network or environment variable (Local-Only Runtime).

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use copybook::framing::encode_all;
use copybook::{
    CobolTimestamp, Count, FixedRecord, Framed, LineFraming, PicDate, PicS, PicTimestamp, PicU,
    PicX, Terminator, TsForm,
};
use domain::batch::loads::{LoadJob, decode_fixture, decode_usrsec, usrsec_instream_lines};
use domain::money::{Money, Rate};
use domain::records::{
    AccountRecord, AuthReply, AuthRequestRecord, CardRecord, CardXrefRecord, CustomerRecord,
    DailyTransactionRecord, DisclosureGroupRecord, ErrorLogRecord, PendingAuthDetailRecord,
    PendingAuthSummaryRecord, RejectRecord, TranCatBalanceRecord, TransactionCategoryRecord,
    TransactionRecord, TransactionTypeRecord, UserSecurityRecord,
};
use domain::sensitive::{CardNumber, Cvv, Password, Ssn};
use proptest::prelude::*;
use rust_decimal::Decimal;

// ---------------------------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------------------------

/// The read-only COBOL tree, `app/`, seen from this crate's manifest directory.
fn app_file(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../app")
        .join(relative);
    std::fs::read(&path).unwrap_or_else(|err| panic!("{relative}: {err}"))
}

/// The bytes of fixture `name` in `app/data/ASCII/`, read in place, every CR kept.
fn fixture(name: &str) -> Vec<u8> {
    app_file(&format!("data/ASCII/{name}"))
}

/// The bytes of `app/jcl/DUSRSECJ.jcl`, whose lines 35-44 are the USRSEC in-stream records.
fn dusrsecj() -> Vec<u8> {
    app_file("jcl/DUSRSECJ.jcl")
}

/// The fixture file name a load job reads.
fn fixture_name(job: LoadJob) -> &'static str {
    job.fixture_file()
        .unwrap_or_else(|| panic!("{} reads no fixture file", job.subcommand()))
}

/// Every line of fixture `name` decoded through `loads::decode_fixture`. A decode error fails the
/// test with its line number, field, offset and width, never the bytes.
fn decode<R: FixedRecord>(name: &str) -> Vec<Framed<R>> {
    decode_fixture::<R>(&fixture(name)).unwrap_or_else(|err| panic!("{name}: {err}"))
}

/// The lines of `bytes`, each with its terminator, split at every LF independently of
/// `copybook::lines`.
fn physical_lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|&byte| byte == b'\n').collect()
}

/// The terminator a physical line ends with, read from its last bytes.
fn terminator_of(line: &[u8]) -> Terminator {
    if line.ends_with(b"\r\n") {
        Terminator::CrLf
    } else if line.ends_with(b"\n") {
        Terminator::Lf
    } else {
        Terminator::None
    }
}

/// A line number counted from 1 as a `PIC 9(n)` value.
fn ordinal(number: usize) -> u64 {
    u64::try_from(number).unwrap_or_else(|_| panic!("line {number} does not fit u64"))
}

/// The calendar date `year-month-day`, which every caller names validly.
fn ymd(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day)
        .unwrap_or_else(|| panic!("{year}-{month}-{day} is not a calendar date"))
}

/// The amount `cents / 100` at scale 2.
fn amount(cents: i64) -> Decimal {
    Decimal::new(cents, 2)
}

/// The shape a fixture file has at `59cc6c2`: its line count, its body length and the terminator
/// of each line by line number (`modern/audit/fixtures.md`, FIXFRM-001 to FIXFRM-013).
struct Shape {
    lines: usize,
    body_len: usize,
    terminator: fn(usize) -> Terminator,
}

/// Every line ends LF.
fn all_lf(_: usize) -> Terminator {
    Terminator::Lf
}

/// Every line ends CRLF.
fn all_crlf(_: usize) -> Terminator {
    Terminator::CrLf
}

/// Decodes fixture `name` through `loads::decode_fixture` and proves byte identity line by line:
/// the record count, each line's framing against `shape` and against its own last bytes, each
/// `Framed::encode` and `Framed::try_encode` against the fixture line with its terminator, and
/// the re-encoded file against the whole file. Returns the decoded records.
fn round_trip_fixture<R: FixedRecord>(name: &str, shape: &Shape) -> Vec<Framed<R>> {
    let bytes = fixture(name);
    let records = decode_fixture::<R>(&bytes).unwrap_or_else(|err| panic!("{name}: {err}"));
    let physical = physical_lines(&bytes);
    assert_eq!(records.len(), shape.lines, "{name}: decoded record count");
    assert_eq!(physical.len(), shape.lines, "{name}: physical line count");
    for (index, (framed, &line)) in records.iter().zip(&physical).enumerate() {
        let number = index + 1;
        let expected = LineFraming {
            terminator: (shape.terminator)(number),
            body_len: shape.body_len,
        };
        assert_eq!(framed.framing, expected, "{name}:{number}: framing");
        assert_eq!(
            terminator_of(line),
            expected.terminator,
            "{name}:{number}: terminator in the file"
        );
        assert_eq!(
            framed.record.encode().len(),
            R::WIDTH,
            "{name}:{number}: record width"
        );
        let encoded = framed.encode();
        assert!(
            encoded == line,
            "{name}:{number}: the re-encoded line differs from the fixture line"
        );
        let checked = framed
            .try_encode()
            .unwrap_or_else(|err| panic!("{name}:{number}: try_encode: {err}"));
        assert!(
            checked == encoded,
            "{name}:{number}: try_encode differs from encode"
        );
    }
    assert!(
        encode_all(&records) == bytes,
        "{name}: the re-encoded file differs from the fixture file"
    );
    records
}

/// The line number, counted from 1, of each record.
fn numbered<R>(records: &[Framed<R>]) -> impl Iterator<Item = (usize, &R)> {
    records
        .iter()
        .enumerate()
        .map(|(index, framed)| (index + 1, &framed.record))
}

/// The record decoded from line `number` of fixture `name`.
fn record_at<'r, R>(name: &str, records: &'r [Framed<R>], number: usize) -> &'r R {
    records.get(number - 1).map_or_else(
        || panic!("{name} has no line {number}"),
        |framed| &framed.record,
    )
}

/// Whether every byte of `field` is `byte`.
fn filled_with<const N: usize>(field: &PicX<N>, byte: u8) -> bool {
    field.as_bytes().iter().all(|&each| each == byte)
}

/// `date` at `hour:minute:second`, which every caller names validly.
fn at(date: NaiveDate, hour: u32, minute: u32, second: u32) -> NaiveDateTime {
    let time = NaiveTime::from_hms_opt(hour, minute, second)
        .unwrap_or_else(|| panic!("{hour}:{minute}:{second} is not a time of day"));
    date.and_time(time)
}

// ---------------------------------------------------------------------------------------------
// Byte identity of the fixture corpus, one test per file (i8)
// ---------------------------------------------------------------------------------------------

// audit: FIXFRM-001
// audit: FIXFRM-014
// audit: FIXLOAD-001
// audit: PIC-014
#[test]
fn acctdata_lines_round_trip_byte_for_byte() {
    let name = fixture_name(LoadJob::Acctfile);
    let shape = Shape {
        lines: 50,
        body_len: 300,
        terminator: all_lf,
    };
    let records = round_trip_fixture::<AccountRecord>(name, &shape);
    for (number, account) in numbered(&records) {
        assert_eq!(
            account.id.get(),
            ordinal(number),
            "{name}:{number}: ACCT-ID"
        );
        let dates = [
            ("ACCT-OPEN-DATE", account.open_date),
            ("ACCT-EXPIRAION-DATE", account.expiraion_date),
            ("ACCT-REISSUE-DATE", account.reissue_date),
        ];
        for (field, date) in dates {
            assert!(date.value().is_some(), "{name}:{number}: {field} is blank");
        }
        assert_eq!(
            account.addr_zip.as_bytes(),
            b"A000000000",
            "{name}:{number}: ACCT-ADDR-ZIP"
        );
        assert!(
            account.group_id.is_spaces(),
            "{name}:{number}: ACCT-GROUP-ID"
        );
        assert!(account.filler.is_spaces(), "{name}:{number}: FILLER");
    }
    // acctdata.txt:1 `00000000001Y00000001940{00000020200{00000010200{2014-11-20...`
    let first = record_at(name, &records, 1);
    assert_eq!(first.active_status.as_bytes(), b"Y");
    assert_eq!(first.curr_bal.as_decimal(), amount(19_400));
    assert_eq!(first.credit_limit.as_decimal(), amount(202_000));
    assert_eq!(first.cash_credit_limit.as_decimal(), amount(102_000));
    assert_eq!(first.open_date.value(), Some(ymd(2014, 11, 20)));
    assert_eq!(first.expiraion_date.value(), Some(ymd(2025, 5, 20)));
    assert_eq!(first.reissue_date.value(), Some(ymd(2025, 5, 20)));
    assert!(first.curr_cyc_credit.is_zero());
    assert!(first.curr_cyc_debit.is_zero());
}

// audit: FIXFRM-002
// audit: FIXLOAD-002
#[test]
fn carddata_lines_round_trip_byte_for_byte() {
    let name = fixture_name(LoadJob::Cardfile);
    let shape = Shape {
        lines: 50,
        body_len: 150,
        terminator: all_lf,
    };
    let records = round_trip_fixture::<CardRecord>(name, &shape);
    for (number, card) in numbered(&records) {
        assert!(
            card.expiraion_date.value().is_some(),
            "{name}:{number}: CARD-EXPIRAION-DATE is blank"
        );
        assert_eq!(
            card.active_status.as_bytes(),
            b"Y",
            "{name}:{number}: CARD-ACTIVE-STATUS"
        );
        assert!(card.filler.is_spaces(), "{name}:{number}: FILLER");
    }
    assert!(
        records
            .windows(2)
            .all(|pair| matches!(pair, [before, after] if before.record.num < after.record.num)),
        "{name}: CARD-NUM keys are not unique and ascending"
    );
    let first = record_at(name, &records, 1);
    assert_eq!(first.acct_id.get(), 50);
    assert_eq!(first.embossed_name.to_text(), "Aniya Von");
    assert_eq!(first.expiraion_date.value(), Some(ymd(2023, 3, 9)));
}

// audit: FIXFRM-003
// audit: FIXFRM-011
// audit: FIXLOAD-003
// audit: PIC-013
#[test]
fn cardxref_short_bodies_round_trip_at_36_bytes() {
    let name = fixture_name(LoadJob::Xreffile);
    let shape = Shape {
        lines: 50,
        body_len: 36,
        terminator: all_lf,
    };
    let records = round_trip_fixture::<CardXrefRecord>(name, &shape);
    let mut accounts = BTreeSet::new();
    for (index, framed) in records.iter().enumerate() {
        let number = index + 1;
        let xref = &framed.record;
        assert!(
            filled_with(&xref.filler, b' '),
            "{name}:{number}: the absent FILLER decodes as 14 spaces"
        );
        assert_eq!(framed.encode().len(), 37, "{name}:{number}: 36 bytes + LF");
        assert!(
            xref.encode()
                .get(36..)
                .is_some_and(|tail| tail.iter().all(|&byte| byte == b' ')),
            "{name}:{number}: the 50-byte record ends with 14 spaces"
        );
        assert_eq!(
            xref.cust_id.get(),
            xref.acct_id.get(),
            "{name}:{number}: XREF-CUST-ID equals XREF-ACCT-ID"
        );
        assert!(
            accounts.insert(xref.acct_id.get()),
            "{name}:{number}: XREF-ACCT-ID repeats"
        );
    }
    assert_eq!(accounts, (1..=50).collect::<BTreeSet<u64>>());
    let first = record_at(name, &records, 1);
    assert_eq!(first.cust_id.get(), 50);
    assert_eq!(first.acct_id.get(), 50);
}

// audit: FIXFRM-004
// audit: FIXLOAD-004
#[test]
fn custdata_lines_round_trip_byte_for_byte() {
    let name = fixture_name(LoadJob::Custfile);
    let shape = Shape {
        lines: 50,
        body_len: 500,
        terminator: all_lf,
    };
    let records = round_trip_fixture::<CustomerRecord>(name, &shape);
    for (number, customer) in numbered(&records) {
        assert_eq!(
            customer.id.get(),
            ordinal(number),
            "{name}:{number}: CUST-ID"
        );
        assert!(
            customer.dob_yyyy_mm_dd.value().is_some(),
            "{name}:{number}: CUST-DOB-YYYY-MM-DD is blank"
        );
        assert!(customer.filler.is_spaces(), "{name}:{number}: FILLER");
    }
    let first = record_at(name, &records, 1);
    assert_eq!(first.first_name.to_text(), "Immanuel");
    assert_eq!(first.middle_name.to_text(), "Madeline");
    assert_eq!(first.last_name.to_text(), "Kessler");
    assert_eq!(first.addr_state_cd.as_bytes(), b"NC");
    assert_eq!(first.addr_country_cd.as_bytes(), b"USA");
    assert_eq!(first.addr_zip.to_text(), "12546");
    assert_eq!(first.dob_yyyy_mm_dd.value(), Some(ymd(1961, 6, 8)));
    assert_eq!(first.pri_card_holder_ind.as_bytes(), b"Y");
    assert_eq!(first.fico_credit_score.get(), 274);
}

/// `dailytran.txt:1`: a `01`/`0001` purchase of 504.77 (`0000005047G`) at merchant `800000000`.
fn assert_first_daily_transaction(first: &DailyTransactionRecord) {
    assert_eq!(first.type_cd.as_bytes(), b"01");
    assert_eq!(first.cat_cd.get(), 1);
    assert_eq!(first.source.to_text(), "POS TERM");
    assert_eq!(first.desc.to_text(), "Purchase at Abshire-Lowe");
    assert_eq!(first.amt.as_decimal(), amount(50_477));
    assert_eq!(first.merchant_id.get(), 800_000_000);
    assert_eq!(first.merchant_name.to_text(), "Abshire-Lowe");
    assert_eq!(first.merchant_city.to_text(), "North Enoshaven");
    assert_eq!(first.merchant_zip.to_text(), "72112");
}

// audit: FIXFRM-005
// audit: FIXFRM-017
// audit: PIC-017
// audit: PIC-018
#[test]
fn dailytran_lines_round_trip_with_typed_timestamps() {
    let name = "dailytran.txt";
    let shape = Shape {
        lines: 300,
        body_len: 350,
        terminator: all_lf,
    };
    let records = round_trip_fixture::<DailyTransactionRecord>(name, &shape);
    let origin = at(ymd(2022, 6, 10), 19, 27, 53);
    let (mut purchases, mut credits) = (0_usize, 0_usize);
    for (number, tran) in numbered(&records) {
        let stamp = tran
            .orig_ts
            .value()
            .unwrap_or_else(|| panic!("{name}:{number}: DALYTRAN-ORIG-TS is blank"));
        assert_eq!(stamp.value(), origin, "{name}:{number}: DALYTRAN-ORIG-TS");
        assert_eq!(stamp.form(), TsForm::Iso, "{name}:{number}: layout I");
        assert_eq!(
            tran.proc_ts.value(),
            None,
            "{name}:{number}: DALYTRAN-PROC-TS"
        );
        assert!(tran.filler.is_spaces(), "{name}:{number}: FILLER");
        assert_eq!(tran.cat_cd.get(), 1, "{name}:{number}: DALYTRAN-CAT-CD");
        let (kind, source) = (tran.type_cd.as_bytes(), tran.source.to_text());
        match (kind, source.as_str()) {
            (b"01", "POS TERM") => purchases += 1,
            (b"03", "OPERATOR") => credits += 1,
            _ => panic!("{name}:{number}: unexpected type {kind:?} and source {source}"),
        }
        assert_eq!(
            tran.amt.is_negative(),
            kind == b"03",
            "{name}:{number}: DALYTRAN-AMT is negative exactly on type 03"
        );
    }
    assert_eq!((purchases, credits), (250, 50), "{name}: types 01 and 03");
    assert!(
        records
            .windows(2)
            .all(|pair| matches!(pair, [before, after] if before.record.id < after.record.id)),
        "{name}: DALYTRAN-ID values are not unique and ascending"
    );
    let cards: BTreeSet<CardNumber> = records.iter().map(|f| f.record.card_num.clone()).collect();
    let xref_cards: BTreeSet<CardNumber> = decode::<CardXrefRecord>("cardxref.txt")
        .into_iter()
        .map(|framed| framed.record.card_num)
        .collect();
    assert_eq!(cards.len(), 50, "{name}: distinct DALYTRAN-CARD-NUM values");
    assert!(
        cards == xref_cards,
        "{name}: the card numbers are not those of cardxref.txt"
    );
    assert_first_daily_transaction(record_at(name, &records, 1));
}

// audit: FIXFRM-006
// audit: FIXFRM-012
// audit: FIXLOAD-005
// audit: PIC-012
#[test]
fn discgrp_lines_round_trip_with_their_zero_filler() {
    let name = fixture_name(LoadJob::Discgrp);
    let shape = Shape {
        lines: 51,
        body_len: 50,
        terminator: all_lf,
    };
    let records = round_trip_fixture::<DisclosureGroupRecord>(name, &shape);
    let mut keys_by_group: [Vec<(PicX<2>, PicU<4>)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (number, group) in numbered(&records) {
        let (slot, expected): (usize, &[u8; 10]) = match number {
            1..=17 => (0, b"A000000000"),
            18..=34 => (1, b"DEFAULT   "),
            _ => (2, b"ZEROAPR   "),
        };
        assert_eq!(
            group.acct_group_id.as_bytes(),
            expected,
            "{name}:{number}: DIS-ACCT-GROUP-ID"
        );
        assert!(
            filled_with(&group.filler, b'0'),
            "{name}:{number}: FILLER is 28 `0` characters (E05)"
        );
        if slot == 2 {
            assert!(group.int_rate.is_zero(), "{name}:{number}: ZEROAPR rate");
        }
        if let Some(keys) = keys_by_group.get_mut(slot) {
            keys.push((group.tran_type_cd.clone(), group.tran_cat_cd));
        }
    }
    let [first_group, default_group, zero_group] = &keys_by_group;
    assert_eq!(first_group.len(), 17, "{name}: keys per group");
    assert!(
        first_group == default_group && default_group == zero_group,
        "{name}: every group carries the same 17 type and category keys"
    );
    let purchase_rate = record_at(name, &records, 18);
    assert_eq!(purchase_rate.tran_type_cd.as_bytes(), b"01");
    assert_eq!(purchase_rate.tran_cat_cd.get(), 1);
    assert_eq!(purchase_rate.int_rate.as_decimal(), amount(1_500));
    let credit_rate = record_at(name, &records, 25);
    assert_eq!(credit_rate.tran_type_cd.as_bytes(), b"03");
    assert_eq!(credit_rate.tran_cat_cd.get(), 1);
    assert!(credit_rate.int_rate.is_zero());
}

/// `tcatbal.txt` ends CRLF on lines 1-49 and LF on line 50 (E06).
fn tcatbal_terminator(number: usize) -> Terminator {
    if number <= 49 {
        Terminator::CrLf
    } else {
        Terminator::Lf
    }
}

// audit: FIXFRM-007
// audit: FIXFRM-012
// audit: FIXFRM-013
// audit: FIXLOAD-006
#[test]
fn tcatbal_lines_round_trip_with_their_mixed_terminators() {
    let name = fixture_name(LoadJob::Tcatbalf);
    let shape = Shape {
        lines: 50,
        body_len: 50,
        terminator: tcatbal_terminator,
    };
    let records = round_trip_fixture::<TranCatBalanceRecord>(name, &shape);
    for (number, balance) in numbered(&records) {
        assert_eq!(
            balance.acct_id.get(),
            ordinal(number),
            "{name}:{number}: TRANCAT-ACCT-ID"
        );
        assert_eq!(
            balance.type_cd.as_bytes(),
            b"01",
            "{name}:{number}: TRANCAT-TYPE-CD"
        );
        assert_eq!(balance.cd.get(), 1, "{name}:{number}: TRANCAT-CD");
        assert!(balance.bal.is_zero(), "{name}:{number}: TRAN-CAT-BAL");
        assert!(
            filled_with(&balance.filler, b'0'),
            "{name}:{number}: FILLER is 22 `0` characters (E05)"
        );
    }
}

// audit: FIXFRM-008
// audit: FIXFRM-012
// audit: FIXFRM-013
// audit: FIXLOAD-007
#[test]
fn trancatg_lines_round_trip_with_their_crlf_terminators() {
    let name = fixture_name(LoadJob::Trancatg);
    let shape = Shape {
        lines: 18,
        body_len: 60,
        terminator: all_crlf,
    };
    let records = round_trip_fixture::<TransactionCategoryRecord>(name, &shape);
    for (number, category) in numbered(&records) {
        assert!(
            filled_with(&category.filler, b'0'),
            "{name}:{number}: FILLER is 4 `0` characters (E05)"
        );
    }
    assert!(
        records.windows(2).all(|pair| matches!(
            pair,
            [before, after] if (&before.record.type_cd, before.record.cat_cd)
                < (&after.record.type_cd, after.record.cat_cd)
        )),
        "{name}: keys are not unique and ascending"
    );
    let sales = record_at(name, &records, 1);
    assert_eq!(sales.type_cd.as_bytes(), b"01");
    assert_eq!(sales.cat_cd.get(), 1);
    assert_eq!(sales.type_desc.to_text(), "Regular Sales Draft");
    let credit = record_at(name, &records, 9);
    assert_eq!(credit.type_cd.as_bytes(), b"03");
    assert_eq!(credit.cat_cd.get(), 1);
    assert_eq!(credit.type_desc.to_text(), "Credit to Account");
}

/// `trantype.txt` ends CRLF on lines 1-6 and LF on line 7 (E06).
fn trantype_terminator(number: usize) -> Terminator {
    if number <= 6 {
        Terminator::CrLf
    } else {
        Terminator::Lf
    }
}

// audit: FIXFRM-009
// audit: FIXFRM-012
// audit: FIXFRM-013
// audit: FIXLOAD-008
#[test]
fn trantype_lines_round_trip_with_their_mixed_terminators() {
    let name = fixture_name(LoadJob::Trantype);
    let shape = Shape {
        lines: 7,
        body_len: 60,
        terminator: trantype_terminator,
    };
    let records = round_trip_fixture::<TransactionTypeRecord>(name, &shape);
    for (number, kind) in numbered(&records) {
        assert_eq!(
            kind.tran_type.to_text(),
            format!("{number:02}"),
            "{name}:{number}: TRAN-TYPE"
        );
        assert!(
            filled_with(&kind.filler, b'0'),
            "{name}:{number}: FILLER is 8 `0` characters (E05)"
        );
    }
    assert_eq!(record_at(name, &records, 1).desc.to_text(), "Purchase");
    assert_eq!(record_at(name, &records, 3).desc.to_text(), "Credit");
}

/// The ten in-stream USRSEC records of `DUSRSECJ.jcl:35-44` (FIXUSR-001 to FIXUSR-010): user ID,
/// first name, last name and user type. The password bytes are not reproduced.
const USERS: [(&str, &str, &str, &[u8; 1]); 10] = [
    ("ADMIN001", "MARGARET", "GOLD", b"A"),
    ("ADMIN002", "RUSSELL", "RUSSELL", b"A"),
    ("ADMIN003", "RAYMOND", "WHITMORE", b"A"),
    ("ADMIN004", "EMMANUEL", "CASGRAIN", b"A"),
    ("ADMIN005", "GRANVILLE", "LACHAPELLE", b"A"),
    ("USER0001", "LAWRENCE", "THOMAS", b"U"),
    ("USER0002", "AJITH", "KUMAR", b"U"),
    ("USER0003", "LAURITZ", "ALME", b"U"),
    ("USER0004", "AVERARDO", "MAZZI", b"U"),
    ("USER0005", "LEE", "TING", b"U"),
];

/// The first in-stream line, `DUSRSECJ.jcl:35`, counted from 1.
const FIRST_USRSEC_LINE: usize = 35;

// audit: FIXUSR-001
// audit: FIXUSR-002
// audit: FIXUSR-003
// audit: FIXUSR-004
// audit: FIXUSR-005
// audit: FIXUSR-006
// audit: FIXUSR-007
// audit: FIXUSR-008
// audit: FIXUSR-009
// audit: FIXUSR-010
// audit: FIXLOAD-009
#[test]
fn usrsec_instream_records_round_trip_byte_for_byte() {
    let jcl = dusrsecj();
    let instream = usrsec_instream_lines(&jcl).unwrap_or_else(|err| panic!("DUSRSECJ.jcl: {err}"));
    let records = decode_usrsec(&jcl).unwrap_or_else(|err| panic!("DUSRSECJ.jcl: {err}"));
    let physical = physical_lines(&jcl);
    let deck = physical
        .get(FIRST_USRSEC_LINE - 1..FIRST_USRSEC_LINE - 1 + USERS.len())
        .unwrap_or_else(|| panic!("DUSRSECJ.jcl ends before line 44"));
    assert_eq!(instream.len(), USERS.len(), "in-stream line count");
    assert_eq!(records.len(), USERS.len(), "decoded record count");
    let rows = records.iter().zip(&instream).zip(deck).zip(USERS);
    for (index, (((framed, line), &original), (id, fname, lname, usr_type))) in rows.enumerate() {
        let number = FIRST_USRSEC_LINE + index;
        let expected = LineFraming {
            terminator: Terminator::Lf,
            body_len: 57,
        };
        assert_eq!(framed.framing, expected, "DUSRSECJ.jcl:{number}: framing");
        assert_eq!(
            line.framing, expected,
            "DUSRSECJ.jcl:{number}: in-stream line"
        );
        assert!(
            framed.encode() == original,
            "DUSRSECJ.jcl:{number}: the re-encoded line differs from the in-stream line"
        );
        let mut image = line.body.to_vec();
        image.resize(UserSecurityRecord::WIDTH, b' ');
        assert!(
            framed.record.encode() == image,
            "DUSRSECJ.jcl:{number}: the record does not re-encode to the 80-byte card image"
        );
        let user = &framed.record;
        assert_eq!(user.id.to_text(), id, "DUSRSECJ.jcl:{number}: SEC-USR-ID");
        assert_eq!(
            user.fname.to_text(),
            fname,
            "DUSRSECJ.jcl:{number}: SEC-USR-FNAME"
        );
        assert_eq!(
            user.lname.to_text(),
            lname,
            "DUSRSECJ.jcl:{number}: SEC-USR-LNAME"
        );
        assert_eq!(
            user.usr_type.as_bytes(),
            usr_type,
            "DUSRSECJ.jcl:{number}: SEC-USR-TYPE"
        );
        assert_eq!(
            user.is_admin(),
            usr_type == b"A",
            "DUSRSECJ.jcl:{number}: admin"
        );
        assert!(
            user.filler.is_spaces(),
            "DUSRSECJ.jcl:{number}: SEC-USR-FILLER"
        );
    }
    assert!(
        encode_all(&records) == deck.concat(),
        "DUSRSECJ.jcl: the re-encoded deck differs from lines 35-44"
    );
}

/// The records a load job's fixture decodes to as layout `R`, after checking that the job's
/// record width is `R`'s.
fn load_count<R: FixedRecord>(job: LoadJob) -> usize {
    assert_eq!(
        job.record_width(),
        R::WIDTH,
        "{}: record width",
        job.subcommand()
    );
    decode::<R>(fixture_name(job)).len()
}

// audit: FIXFRM-010
#[test]
fn the_nine_fixture_files_hold_626_records() {
    let mut counts = Vec::new();
    for job in LoadJob::ALL {
        let count = match job {
            LoadJob::Acctfile => load_count::<AccountRecord>(job),
            LoadJob::Cardfile => load_count::<CardRecord>(job),
            LoadJob::Xreffile => load_count::<CardXrefRecord>(job),
            LoadJob::Custfile => load_count::<CustomerRecord>(job),
            LoadJob::Discgrp => load_count::<DisclosureGroupRecord>(job),
            LoadJob::Tcatbalf => load_count::<TranCatBalanceRecord>(job),
            LoadJob::Trancatg => load_count::<TransactionCategoryRecord>(job),
            LoadJob::Trantype => load_count::<TransactionTypeRecord>(job),
            // dusrsecj reads the in-stream records of DUSRSECJ.jcl, which are not among the 626.
            LoadJob::Dusrsecj => continue,
        };
        counts.push(count);
    }
    assert_eq!(counts, [50, 50, 50, 50, 51, 50, 18, 7], "load fixtures");
    let daily = decode::<DailyTransactionRecord>("dailytran.txt").len();
    assert_eq!(daily, 300, "dailytran.txt");
    assert_eq!(
        counts.iter().sum::<usize>() + daily,
        626,
        "the fixture corpus"
    );
}

// audit: FIXFRM-016
// audit: PIC-015
#[test]
fn the_250_fixture_dates_decode_to_calendar_dates() {
    let accounts = decode::<AccountRecord>("acctdata.txt");
    let cards = decode::<CardRecord>("carddata.txt");
    let customers = decode::<CustomerRecord>("custdata.txt");
    let dates: Vec<Option<NaiveDate>> = accounts
        .iter()
        .flat_map(|framed| {
            let account = &framed.record;
            [
                account.open_date,
                account.expiraion_date,
                account.reissue_date,
            ]
        })
        .chain(cards.iter().map(|framed| framed.record.expiraion_date))
        .chain(customers.iter().map(|framed| framed.record.dob_yyyy_mm_dd))
        .map(PicDate::value)
        .collect();
    assert_eq!(dates.len(), 250, "dates in the corpus");
    assert!(
        dates.iter().all(Option::is_some),
        "a fixture date decoded blank"
    );
    let earliest_expiry = accounts
        .iter()
        .filter_map(|framed| framed.record.expiraion_date.value())
        .min();
    assert_eq!(
        earliest_expiry,
        Some(ymd(2023, 1, 6)),
        "ACCT-EXPIRAION-DATE"
    );
}

/// Checks that `debug`, the `Debug` rendering of the record at `place`, shows `card` only in its
/// redacted form, twelve `*` and the last four digits, and that the card number is 16 ASCII
/// digits (FIXFRM-019, PIC-024). Nothing compared here is printed.
fn assert_card_redacted(card: &CardNumber, debug: &str, place: &str) {
    let raw = card.expose().to_text();
    assert!(
        raw.len() == 16 && raw.bytes().all(|byte| byte.is_ascii_digit()),
        "{place}: the card number is not 16 ASCII digits"
    );
    let last4: String = card.last4().iter().copied().map(char::from).collect();
    let redacted = card.redacted();
    assert!(
        redacted == format!("************{last4}") && raw.ends_with(&last4),
        "{place}: the redacted form is not twelve `*` and the last four digits"
    );
    assert!(
        format!("{card:?}") == redacted && format!("{card}") == redacted,
        "{place}: Debug or Display of the card number is not its redacted form"
    );
    assert!(
        !debug.contains(&raw),
        "{place}: Debug shows the full card number"
    );
    assert!(
        debug.contains(&redacted),
        "{place}: Debug lacks the redacted card number"
    );
}

/// Checks that `debug`, the `Debug` rendering of the record at `place`, shows `ssn` only as
/// `***-**-` and its last four digits, with or without dashes elsewhere (PIC-024).
fn assert_ssn_redacted(ssn: Ssn, debug: &str, place: &str) {
    let value = ssn.expose().get();
    let plain = format!("{value:09}");
    let dashed = format!(
        "{:03}-{:02}-{:04}",
        value / 1_000_000,
        value / 10_000 % 100,
        value % 10_000
    );
    let redacted = ssn.redacted();
    assert!(
        redacted == format!("***-**-{:04}", value % 10_000),
        "{place}: the redacted form is not `***-**-` and the last four digits"
    );
    assert!(
        format!("{ssn:?}") == redacted && format!("{ssn}") == redacted,
        "{place}: Debug or Display of the SSN is not its redacted form"
    );
    assert!(
        !debug.contains(&plain) && !debug.contains(&dashed),
        "{place}: Debug shows the full SSN"
    );
}

// audit: FIXFRM-019
// audit: FIXFRM-021
// audit: PIC-024
#[test]
fn decoded_records_show_sensitive_fields_only_redacted() {
    for (number, card) in numbered(&decode::<CardRecord>("carddata.txt")) {
        let place = format!("carddata.txt:{number}");
        let debug = format!("{card:?}");
        assert_card_redacted(&card.num, &debug, &place);
        assert!(
            format!("{:?}", card.cvv_cd) == "***" && format!("{}", card.cvv_cd) == "***",
            "{place}: Debug or Display of CARD-CVV-CD is not redacted"
        );
    }
    for (number, xref) in numbered(&decode::<CardXrefRecord>("cardxref.txt")) {
        let place = format!("cardxref.txt:{number}");
        assert_card_redacted(&xref.card_num, &format!("{xref:?}"), &place);
    }
    for (number, tran) in numbered(&decode::<DailyTransactionRecord>("dailytran.txt")) {
        let place = format!("dailytran.txt:{number}");
        assert_card_redacted(&tran.card_num, &format!("{tran:?}"), &place);
    }
    for (number, customer) in numbered(&decode::<CustomerRecord>("custdata.txt")) {
        let place = format!("custdata.txt:{number}");
        assert_ssn_redacted(customer.ssn, &format!("{customer:?}"), &place);
    }
    let users = decode_usrsec(&dusrsecj()).unwrap_or_else(|err| panic!("DUSRSECJ.jcl: {err}"));
    for (index, user) in numbered(&users) {
        let place = format!("DUSRSECJ.jcl:{}", FIRST_USRSEC_LINE + index - 1);
        let raw: String = user.pwd.expose().iter().copied().map(char::from).collect();
        assert!(
            format!("{:?}", user.pwd) == "********" && format!("{}", user.pwd) == "********",
            "{place}: Debug or Display of SEC-USR-PWD is not redacted"
        );
        assert!(
            !format!("{user:?}").contains(&raw),
            "{place}: Debug shows SEC-USR-PWD"
        );
    }
}

/// The decoded corpus in file order, the nine fixture files and then the USRSEC deck: the value
/// the snapshot fixes (FIXFRM-010). Its `Debug` renders card numbers, SSNs, CVVs and passwords
/// only in their redacted forms (PIC-024).
#[derive(Debug)]
struct DecodedCorpus {
    acctdata: Vec<Framed<AccountRecord>>,
    carddata: Vec<Framed<CardRecord>>,
    cardxref: Vec<Framed<CardXrefRecord>>,
    custdata: Vec<Framed<CustomerRecord>>,
    dailytran: Vec<Framed<DailyTransactionRecord>>,
    discgrp: Vec<Framed<DisclosureGroupRecord>>,
    tcatbal: Vec<Framed<TranCatBalanceRecord>>,
    trancatg: Vec<Framed<TransactionCategoryRecord>>,
    trantype: Vec<Framed<TransactionTypeRecord>>,
    usrsec: Vec<Framed<UserSecurityRecord>>,
}

impl DecodedCorpus {
    /// Every file decoded in place.
    fn decode() -> Self {
        Self {
            acctdata: decode("acctdata.txt"),
            carddata: decode("carddata.txt"),
            cardxref: decode("cardxref.txt"),
            custdata: decode("custdata.txt"),
            dailytran: decode("dailytran.txt"),
            discgrp: decode("discgrp.txt"),
            tcatbal: decode("tcatbal.txt"),
            trancatg: decode("trancatg.txt"),
            trantype: decode("trantype.txt"),
            usrsec: decode_usrsec(&dusrsecj()).unwrap_or_else(|err| panic!("DUSRSECJ.jcl: {err}")),
        }
    }

    /// The records of the nine fixture files, the USRSEC deck excluded.
    fn fixture_records(&self) -> usize {
        [
            self.acctdata.len(),
            self.carddata.len(),
            self.cardxref.len(),
            self.custdata.len(),
            self.dailytran.len(),
            self.discgrp.len(),
            self.tcatbal.len(),
            self.trancatg.len(),
            self.trantype.len(),
        ]
        .into_iter()
        .sum()
    }
}

// audit: FIXFRM-010
#[test]
fn decoded_corpus_matches_its_snapshot() {
    let corpus = DecodedCorpus::decode();
    assert_eq!(corpus.fixture_records(), 626, "fixture records");
    assert_eq!(corpus.usrsec.len(), USERS.len(), "USRSEC records");
    insta::assert_debug_snapshot!("decoded_corpus", corpus);
}

// ---------------------------------------------------------------------------------------------
// Field strategies
// ---------------------------------------------------------------------------------------------

/// The configuration of every property. Failure persistence is off, so no
/// `proptest-regressions/` file is written beside this file.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 256,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// A `PIC X(N)` value: `N` printable ASCII bytes, the bytes the codec accepts (PIC-001).
fn pic_x<const N: usize>() -> impl Strategy<Value = PicX<N>> {
    proptest::array::uniform::<_, N>(0x20_u8..=0x7E).prop_map(PicX::from_array)
}

/// A `PIC 9(N)` value in `0..=10^N - 1`, its bounds weighted in (PIC-003, PIC-023).
fn pic_u<const N: u32>() -> impl Strategy<Value = PicU<N>> {
    let max = PicU::<N>::MAX;
    prop_oneof![
        1 => Just(0),
        1 => Just(max),
        8 => 0..=max,
    ]
    .prop_filter_map("a PIC 9(n) value", |value| PicU::<N>::new(value).ok())
}

/// A `PIC S9(N)` value in `-(10^N - 1)..=10^N - 1`, its bounds weighted in (PIC-010, PIC-023).
fn pic_s<const N: u32>() -> impl Strategy<Value = PicS<N>> {
    let max = PicS::<N>::MAX;
    prop_oneof![
        1 => Just(-max),
        1 => Just(0),
        1 => Just(max),
        7 => -max..=max,
    ]
    .prop_filter_map("a PIC S9(n) value", |value| PicS::<N>::new(value).ok())
}

/// A `V99` decimal with at most `int_digits` integer digits, its bounds weighted in.
fn scaled(int_digits: u32) -> impl Strategy<Value = Decimal> {
    let max = 10_i64.pow(int_digits + 2) - 1;
    prop_oneof![
        1 => Just(-max),
        1 => Just(0),
        1 => Just(max),
        7 => -max..=max,
    ]
    .prop_map(amount)
}

/// A `PIC S9(N)V99` amount, DISPLAY, `COMP-3` or edited (PIC-005, PIC-010, PIC-022).
fn money<const N: u32>() -> impl Strategy<Value = Money<N>> {
    scaled(N).prop_filter_map("a S9(n)V99 amount", |value| Money::<N>::new(value).ok())
}

/// A `DIS-INT-RATE PIC S9(04)V99` rate (CVTRA02Y-004).
fn rate() -> impl Strategy<Value = Rate<4>> {
    scaled(4).prop_filter_map("a S9(04)V99 rate", |value| Rate::<4>::new(value).ok())
}

/// A calendar date with a four-digit year, `0000-01-01` to `9999-12-31`.
fn calendar_date() -> impl Strategy<Value = NaiveDate> {
    (0_i32..=9999, 1_u32..=366).prop_filter_map("a day of that year", |(year, ordinal)| {
        NaiveDate::from_yo_opt(year, ordinal)
    })
}

/// A time of day with microsecond precision and no leap second.
fn time_of_day() -> impl Strategy<Value = NaiveTime> {
    (0_u32..24, 0_u32..60, 0_u32..60, 0_u32..1_000_000)
        .prop_filter_map("a valid time of day", |(hour, minute, second, micro)| {
            NaiveTime::from_hms_micro_opt(hour, minute, second, micro)
        })
}

/// A `PIC X(10)` date: blank or a calendar date (PIC-015).
fn pic_date() -> impl Strategy<Value = PicDate> {
    prop_oneof![
        1 => Just(PicDate::BLANK),
        4 => calendar_date()
            .prop_filter_map("a four-digit year", |date| PicDate::new(Some(date)).ok()),
    ]
}

/// A `PIC X(26)` timestamp: blank, or a value in layout `I`, `D` or `S`, the screen-date layout
/// carrying a date at midnight only (PIC-017 to PIC-020).
fn pic_timestamp() -> impl Strategy<Value = PicTimestamp> {
    let forms = prop::sample::select(vec![TsForm::Iso, TsForm::Db2, TsForm::ScreenDate]);
    prop_oneof![
        1 => Just(PicTimestamp::BLANK),
        4 => (calendar_date(), time_of_day(), forms).prop_filter_map(
            "a timestamp its layout can carry",
            |(date, time, form)| {
                let value = match form {
                    TsForm::ScreenDate => date.and_time(NaiveTime::MIN),
                    TsForm::Iso | TsForm::Db2 => date.and_time(time),
                };
                CobolTimestamp::new(value, form)
                    .ok()
                    .map(|stamp| PicTimestamp::new(Some(stamp)))
            },
        ),
    ]
}

/// A card number built from 16 ASCII digits through `CardNumber::parse`.
fn card_number() -> impl Strategy<Value = CardNumber> {
    proptest::array::uniform::<_, 16>(b'0'..=b'9').prop_filter_map("16 ASCII digits", |digits| {
        std::str::from_utf8(&digits)
            .ok()
            .and_then(|text| CardNumber::parse(text).ok())
    })
}

/// A `CUST-SSN PIC 9(09)` value.
fn ssn() -> impl Strategy<Value = Ssn> {
    pic_u::<9>().prop_map(Ssn::new)
}

/// A `CARD-CVV-CD PIC 9(03)` value.
fn cvv() -> impl Strategy<Value = Cvv> {
    pic_u::<3>().prop_map(Cvv::new)
}

/// A `SEC-USR-PWD PIC X(08)` value.
fn password() -> impl Strategy<Value = Password> {
    pic_x::<8>().prop_map(Password::new)
}

/// A `PIC S9(4) COMP` count: every halfword (PIC-011).
fn count() -> impl Strategy<Value = Count> {
    any::<i16>().prop_map(Count::new)
}

/// The terminator of a generated fixture line.
fn terminator() -> impl Strategy<Value = Terminator> {
    prop::sample::select(vec![Terminator::Lf, Terminator::CrLf, Terminator::None])
}

// ---------------------------------------------------------------------------------------------
// Record strategies, one per FixedRecord layout
// ---------------------------------------------------------------------------------------------

/// `CVACT01Y` `ACCOUNT-RECORD`.
fn account_strategy() -> impl Strategy<Value = AccountRecord> {
    (
        (pic_u(), pic_x(), money(), money(), money()),
        (pic_date(), pic_date(), pic_date()),
        (money(), money(), pic_x(), pic_x(), pic_x()),
    )
        .prop_map(
            |(
                (id, active_status, curr_bal, credit_limit, cash_credit_limit),
                (open_date, expiraion_date, reissue_date),
                (curr_cyc_credit, curr_cyc_debit, addr_zip, group_id, filler),
            )| AccountRecord {
                id,
                active_status,
                curr_bal,
                credit_limit,
                cash_credit_limit,
                open_date,
                expiraion_date,
                reissue_date,
                curr_cyc_credit,
                curr_cyc_debit,
                addr_zip,
                group_id,
                filler,
            },
        )
}

/// `CVACT02Y` `CARD-RECORD`.
fn card_strategy() -> impl Strategy<Value = CardRecord> {
    (
        card_number(),
        pic_u(),
        cvv(),
        pic_x(),
        pic_date(),
        pic_x(),
        pic_x(),
    )
        .prop_map(
            |(num, acct_id, cvv_cd, embossed_name, expiraion_date, active_status, filler)| {
                CardRecord {
                    num,
                    acct_id,
                    cvv_cd,
                    embossed_name,
                    expiraion_date,
                    active_status,
                    filler,
                }
            },
        )
}

/// `CVACT03Y` `CARD-XREF-RECORD`.
fn card_xref_strategy() -> impl Strategy<Value = CardXrefRecord> {
    (card_number(), pic_u(), pic_u(), pic_x()).prop_map(|(card_num, cust_id, acct_id, filler)| {
        CardXrefRecord {
            card_num,
            cust_id,
            acct_id,
            filler,
        }
    })
}

/// `CVCUS01Y` `CUSTOMER-RECORD`.
fn customer_strategy() -> impl Strategy<Value = CustomerRecord> {
    (
        (pic_u(), pic_x(), pic_x(), pic_x()),
        (pic_x(), pic_x(), pic_x(), pic_x(), pic_x(), pic_x()),
        (pic_x(), pic_x(), ssn(), pic_x(), pic_date()),
        (pic_x(), pic_x(), pic_u(), pic_x()),
    )
        .prop_map(
            |(
                (id, first_name, middle_name, last_name),
                (addr_line_1, addr_line_2, addr_line_3, addr_state_cd, addr_country_cd, addr_zip),
                (phone_num_1, phone_num_2, ssn, govt_issued_id, dob_yyyy_mm_dd),
                (eft_account_id, pri_card_holder_ind, fico_credit_score, filler),
            )| CustomerRecord {
                id,
                first_name,
                middle_name,
                last_name,
                addr_line_1,
                addr_line_2,
                addr_line_3,
                addr_state_cd,
                addr_country_cd,
                addr_zip,
                phone_num_1,
                phone_num_2,
                ssn,
                govt_issued_id,
                dob_yyyy_mm_dd,
                eft_account_id,
                pri_card_holder_ind,
                fico_credit_score,
                filler,
            },
        )
}

/// `CVTRA05Y` `TRAN-RECORD`; `CVTRA06Y` `DALYTRAN-RECORD` has the same layout.
fn transaction_strategy() -> impl Strategy<Value = TransactionRecord> {
    (
        (pic_x(), pic_x(), pic_u(), pic_x(), pic_x(), money()),
        (pic_u(), pic_x(), pic_x(), pic_x()),
        (card_number(), pic_timestamp(), pic_timestamp(), pic_x()),
    )
        .prop_map(
            |(
                (id, type_cd, cat_cd, source, desc, amt),
                (merchant_id, merchant_name, merchant_city, merchant_zip),
                (card_num, orig_ts, proc_ts, filler),
            )| TransactionRecord {
                id,
                type_cd,
                cat_cd,
                source,
                desc,
                amt,
                merchant_id,
                merchant_name,
                merchant_city,
                merchant_zip,
                card_num,
                orig_ts,
                proc_ts,
                filler,
            },
        )
}

/// The `CBTRN02C` reject record: the 350 input bytes and the 80-byte validation trailer.
fn reject_strategy() -> impl Strategy<Value = RejectRecord> {
    (pic_x(), pic_u(), pic_x()).prop_map(|(tran_data, reason, description)| {
        RejectRecord::new(&tran_data, reason, &description)
    })
}

/// `CVTRA01Y` `TRAN-CAT-BAL-RECORD`.
fn tran_cat_balance_strategy() -> impl Strategy<Value = TranCatBalanceRecord> {
    (pic_u(), pic_x(), pic_u(), money(), pic_x()).prop_map(|(acct_id, type_cd, cd, bal, filler)| {
        TranCatBalanceRecord {
            acct_id,
            type_cd,
            cd,
            bal,
            filler,
        }
    })
}

/// `CVTRA02Y` `DIS-GROUP-RECORD`.
fn disclosure_group_strategy() -> impl Strategy<Value = DisclosureGroupRecord> {
    (pic_x(), pic_x(), pic_u(), rate(), pic_x()).prop_map(
        |(acct_group_id, tran_type_cd, tran_cat_cd, int_rate, filler)| DisclosureGroupRecord {
            acct_group_id,
            tran_type_cd,
            tran_cat_cd,
            int_rate,
            filler,
        },
    )
}

/// `CVTRA03Y` `TRAN-TYPE-RECORD`.
fn transaction_type_strategy() -> impl Strategy<Value = TransactionTypeRecord> {
    (pic_x(), pic_x(), pic_x()).prop_map(|(tran_type, desc, filler)| TransactionTypeRecord {
        tran_type,
        desc,
        filler,
    })
}

/// `CVTRA04Y` `TRAN-CAT-RECORD`.
fn transaction_category_strategy() -> impl Strategy<Value = TransactionCategoryRecord> {
    (pic_x(), pic_u(), pic_x(), pic_x()).prop_map(|(type_cd, cat_cd, type_desc, filler)| {
        TransactionCategoryRecord {
            type_cd,
            cat_cd,
            type_desc,
            filler,
        }
    })
}

/// `CSUSR01Y` `SEC-USER-DATA`.
fn user_security_strategy() -> impl Strategy<Value = UserSecurityRecord> {
    (pic_x(), pic_x(), pic_x(), password(), pic_x(), pic_x()).prop_map(
        |(id, fname, lname, pwd, usr_type, filler)| UserSecurityRecord {
            id,
            fname,
            lname,
            pwd,
            usr_type,
            filler,
        },
    )
}

/// `CIPAUSMY` `PENDING-AUTH-SUMMARY`, IMS segment `PAUTSUM0`.
fn pending_auth_summary_strategy() -> impl Strategy<Value = PendingAuthSummaryRecord> {
    (
        (pic_s(), pic_u(), pic_x(), proptest::array::uniform(pic_x())),
        (money(), money(), money(), money()),
        (count(), count(), money(), money(), pic_x()),
    )
        .prop_map(
            |(
                (acct_id, cust_id, auth_status, account_status),
                (credit_limit, cash_limit, credit_balance, cash_balance),
                (
                    approved_auth_cnt,
                    declined_auth_cnt,
                    approved_auth_amt,
                    declined_auth_amt,
                    filler,
                ),
            )| PendingAuthSummaryRecord {
                acct_id,
                cust_id,
                auth_status,
                account_status,
                credit_limit,
                cash_limit,
                credit_balance,
                cash_balance,
                approved_auth_cnt,
                declined_auth_cnt,
                approved_auth_amt,
                declined_auth_amt,
                filler,
            },
        )
}

/// The authorization fields of `CIPAUDTY`: `PA-AUTH-DATE-9C` to `PA-AUTH-RESP-REASON`.
type DetailAuthorization = (
    PicS<5>,
    PicS<9>,
    PicX<6>,
    PicX<6>,
    CardNumber,
    PicX<4>,
    PicX<4>,
    PicX<6>,
    PicX<6>,
    PicX<6>,
    PicX<2>,
    PicX<4>,
);

/// The merchant and status fields of `CIPAUDTY`: `PA-TRANSACTION-AMT` to `FILLER`.
type DetailMerchant = (
    (Money<10>, Money<10>, PicX<4>, PicX<3>, PicU<2>),
    (PicX<15>, PicX<22>, PicX<13>, PicX<2>, PicX<9>, PicX<15>),
    (PicX<1>, PicX<1>, PicX<8>, PicX<17>),
);

/// `PA-AUTH-DATE-9C` to `PA-AUTH-RESP-REASON` of `CIPAUDTY`.
fn detail_authorization_strategy() -> impl Strategy<Value = DetailAuthorization> {
    (
        pic_s(),
        pic_s(),
        pic_x(),
        pic_x(),
        card_number(),
        pic_x(),
        pic_x(),
        pic_x(),
        pic_x(),
        pic_x(),
        pic_x(),
        pic_x(),
    )
}

/// `PA-TRANSACTION-AMT` to `FILLER` of `CIPAUDTY`.
fn detail_merchant_strategy() -> impl Strategy<Value = DetailMerchant> {
    (
        (money(), money(), pic_x(), pic_x(), pic_u()),
        (pic_x(), pic_x(), pic_x(), pic_x(), pic_x(), pic_x()),
        (pic_x(), pic_x(), pic_x(), pic_x()),
    )
}

/// `CIPAUDTY` `PENDING-AUTH-DETAILS`, IMS segment `PAUTDTL1`.
fn pending_auth_detail_strategy() -> impl Strategy<Value = PendingAuthDetailRecord> {
    (
        detail_authorization_strategy(),
        pic_u(),
        detail_merchant_strategy(),
    )
        .prop_map(
            |(
                (
                    auth_date_9c,
                    auth_time_9c,
                    orig_date,
                    orig_time,
                    card_num,
                    auth_type,
                    card_expiry_date,
                    message_type,
                    message_source,
                    auth_id_code,
                    auth_resp_code,
                    auth_resp_reason,
                ),
                processing_code,
                (
                    (
                        transaction_amt,
                        approved_amt,
                        merchant_catagory_code,
                        acqr_country_code,
                        pos_entry_mode,
                    ),
                    (
                        merchant_id,
                        merchant_name,
                        merchant_city,
                        merchant_state,
                        merchant_zip,
                        transaction_id,
                    ),
                    (match_status, auth_fraud, fraud_rpt_date, filler),
                ),
            )| PendingAuthDetailRecord {
                auth_date_9c,
                auth_time_9c,
                orig_date,
                orig_time,
                card_num,
                auth_type,
                card_expiry_date,
                message_type,
                message_source,
                auth_id_code,
                auth_resp_code,
                auth_resp_reason,
                processing_code,
                transaction_amt,
                approved_amt,
                merchant_catagory_code,
                acqr_country_code,
                pos_entry_mode,
                merchant_id,
                merchant_name,
                merchant_city,
                merchant_state,
                merchant_zip,
                transaction_id,
                match_status,
                auth_fraud,
                fraud_rpt_date,
                filler,
            },
        )
}

/// `CCPAURQY` `PENDING-AUTH-REQUEST`, the CP00 request storage layout.
fn auth_request_strategy() -> impl Strategy<Value = AuthRequestRecord> {
    (
        (pic_x(), pic_x(), card_number(), pic_x(), pic_x(), pic_x()),
        (pic_x(), pic_u(), money(), pic_x(), pic_x(), pic_u()),
        (pic_x(), pic_x(), pic_x(), pic_x(), pic_x(), pic_x()),
    )
        .prop_map(
            |(
                (auth_date, auth_time, card_num, auth_type, card_expiry_date, message_type),
                (
                    message_source,
                    processing_code,
                    transaction_amt,
                    merchant_catagory_code,
                    acqr_country_code,
                    pos_entry_mode,
                ),
                (
                    merchant_id,
                    merchant_name,
                    merchant_city,
                    merchant_state,
                    merchant_zip,
                    transaction_id,
                ),
            )| AuthRequestRecord {
                auth_date,
                auth_time,
                card_num,
                auth_type,
                card_expiry_date,
                message_type,
                message_source,
                processing_code,
                transaction_amt,
                merchant_catagory_code,
                acqr_country_code,
                pos_entry_mode,
                merchant_id,
                merchant_name,
                merchant_city,
                merchant_state,
                merchant_zip,
                transaction_id,
            },
        )
}

/// `CCPAURLY` `PENDING-AUTH-RESPONSE`, the CP00 reply storage layout.
fn auth_reply_strategy() -> impl Strategy<Value = AuthReply> {
    (card_number(), pic_x(), pic_x(), pic_x(), pic_x(), money()).prop_map(
        |(
            card_num,
            transaction_id,
            auth_id_code,
            auth_resp_code,
            auth_resp_reason,
            approved_amt,
        )| {
            AuthReply {
                card_num,
                transaction_id,
                auth_id_code,
                auth_resp_code,
                auth_resp_reason,
                approved_amt,
            }
        },
    )
}

/// `CCPAUERY` `ERROR-LOG-RECORD`.
fn error_log_strategy() -> impl Strategy<Value = ErrorLogRecord> {
    (
        (pic_x(), pic_x(), pic_x(), pic_x(), pic_x(), pic_x()),
        (pic_x(), pic_x(), pic_x(), pic_x(), pic_x()),
    )
        .prop_map(
            |(
                (date, time, application, program, location, level),
                (subsystem, code_1, code_2, message, event_key),
            )| ErrorLogRecord {
                date,
                time,
                application,
                program,
                location,
                level,
                subsystem,
                code_1,
                code_2,
                message,
                event_key,
            },
        )
}

// ---------------------------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------------------------

/// `decode(encode(r)) == r` for one record: the encoding is exactly `WIDTH` bytes, decodes back
/// to an equal record, and that record re-encodes to the same bytes. A failure names the layout
/// width only; proptest reports the generated record through its `Debug`, which redacts every
/// sensitive field.
fn record_round_trip<R: FixedRecord + PartialEq>(record: &R) -> Result<(), TestCaseError> {
    let bytes = record.encode();
    prop_assert_eq!(bytes.len(), R::WIDTH);
    let back = R::decode(&bytes)
        .map_err(|err| TestCaseError::fail(format!("decode(encode(r)) failed: {err}")))?;
    prop_assert!(back == *record, "decode(encode(r)) differs from r");
    prop_assert!(
        back.encode() == bytes,
        "encode(decode(bytes)) differs from bytes"
    );
    Ok(())
}

/// The `Framed` round trip of one record on a fixture line through `loads::decode_fixture`: the
/// line is the record's bytes with the last `cut` bytes dropped (`cut` at most the record's
/// trailing `FILLER`, PIC-013) and then `terminator` (PIC-014). The line decodes to one record
/// whose framing is that terminator and body length, `encode` and `try_encode` give the line
/// back, and the record is the original one, or for a short body the original with its dropped
/// tail blank.
fn framed_round_trip<R: FixedRecord + PartialEq>(
    record: &R,
    terminator: Terminator,
    cut: usize,
) -> Result<(), TestCaseError> {
    let bytes = record.encode();
    let body_len = R::WIDTH
        .checked_sub(cut)
        .ok_or_else(|| TestCaseError::fail("cut exceeds the record width"))?;
    let body = bytes
        .get(..body_len)
        .ok_or_else(|| TestCaseError::fail("the encoding is shorter than WIDTH"))?;
    let mut line = body.to_vec();
    line.extend_from_slice(terminator.as_bytes());
    let decoded = decode_fixture::<R>(&line)
        .map_err(|err| TestCaseError::fail(format!("decode_fixture: {err}")))?;
    let [framed] = decoded.as_slice() else {
        return Err(TestCaseError::fail("the line did not decode to one record"));
    };
    prop_assert_eq!(
        framed.framing,
        LineFraming {
            terminator,
            body_len
        }
    );
    prop_assert!(
        framed.encode() == line,
        "Framed::encode differs from the line"
    );
    let checked = framed
        .try_encode()
        .map_err(|err| TestCaseError::fail(format!("Framed::try_encode: {err}")))?;
    prop_assert!(checked == line, "Framed::try_encode differs from the line");
    if cut == 0 {
        prop_assert!(framed.record == *record, "the framed record differs from r");
    } else {
        let mut padded = body.to_vec();
        padded.resize(R::WIDTH, b' ');
        prop_assert!(
            framed.record.encode() == padded,
            "a short body does not decode with a blank tail"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// decode(encode(r)) == r per layout (i8)
// ---------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(config())]

    // audit: CVACT01Y-001
    // audit: CVACT01Y-002
    // audit: CVACT01Y-003
    // audit: CVACT01Y-004
    // audit: CVACT01Y-005
    // audit: CVACT01Y-006
    // audit: CVACT01Y-007
    // audit: CVACT01Y-008
    // audit: CVACT01Y-009
    // audit: CVACT01Y-010
    // audit: CVACT01Y-011
    // audit: CVACT01Y-012
    // audit: CVACT01Y-013
    #[test]
    fn account_records_round_trip(
        record in account_strategy(),
        terminator in terminator(),
        cut in 0..=AccountRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    // audit: CVACT02Y-001
    // audit: CVACT02Y-002
    // audit: CVACT02Y-003
    // audit: CVACT02Y-004
    // audit: CVACT02Y-005
    // audit: CVACT02Y-006
    // audit: CVACT02Y-007
    #[test]
    fn card_records_round_trip(
        record in card_strategy(),
        terminator in terminator(),
        cut in 0..=CardRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    // audit: CVACT03Y-001
    // audit: CVACT03Y-002
    // audit: CVACT03Y-003
    // audit: CVACT03Y-004
    // audit: PIC-013
    #[test]
    fn card_xref_records_round_trip(
        record in card_xref_strategy(),
        terminator in terminator(),
        cut in 0..=CardXrefRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    // audit: CVCUS01Y-001
    // audit: CVCUS01Y-002
    // audit: CVCUS01Y-003
    // audit: CVCUS01Y-004
    // audit: CVCUS01Y-005
    // audit: CVCUS01Y-006
    // audit: CVCUS01Y-007
    // audit: CVCUS01Y-008
    // audit: CVCUS01Y-009
    // audit: CVCUS01Y-010
    // audit: CVCUS01Y-011
    // audit: CVCUS01Y-012
    // audit: CVCUS01Y-013
    // audit: CVCUS01Y-014
    // audit: CVCUS01Y-015
    // audit: CVCUS01Y-016
    // audit: CVCUS01Y-017
    // audit: CVCUS01Y-018
    // audit: CVCUS01Y-019
    #[test]
    fn customer_records_round_trip(
        record in customer_strategy(),
        terminator in terminator(),
        cut in 0..=CustomerRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    // audit: CVTRA05Y-001
    // audit: CVTRA05Y-002
    // audit: CVTRA05Y-003
    // audit: CVTRA05Y-004
    // audit: CVTRA05Y-005
    // audit: CVTRA05Y-006
    // audit: CVTRA05Y-007
    // audit: CVTRA05Y-008
    // audit: CVTRA05Y-009
    // audit: CVTRA05Y-010
    // audit: CVTRA05Y-011
    // audit: CVTRA05Y-012
    // audit: CVTRA05Y-013
    // audit: CVTRA05Y-014
    // audit: PIC-017
    // audit: PIC-018
    // audit: PIC-019
    // audit: PIC-020
    #[test]
    fn transaction_records_round_trip(record in transaction_strategy()) {
        record_round_trip(&record)?;
    }

    // audit: CVTRA06Y-001
    // audit: CVTRA06Y-002
    // audit: CVTRA06Y-003
    // audit: CVTRA06Y-004
    // audit: CVTRA06Y-005
    // audit: CVTRA06Y-006
    // audit: CVTRA06Y-007
    // audit: CVTRA06Y-008
    // audit: CVTRA06Y-009
    // audit: CVTRA06Y-010
    // audit: CVTRA06Y-011
    // audit: CVTRA06Y-012
    // audit: CVTRA06Y-013
    // audit: CVTRA06Y-014
    #[test]
    fn daily_transaction_records_round_trip(
        record in transaction_strategy().prop_map(DailyTransactionRecord::from),
        terminator in terminator(),
        cut in 0..=DailyTransactionRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    #[test]
    fn reject_records_round_trip(record in reject_strategy()) {
        record_round_trip(&record)?;
    }

    // audit: CVTRA01Y-001
    // audit: CVTRA01Y-002
    // audit: CVTRA01Y-003
    // audit: CVTRA01Y-004
    // audit: CVTRA01Y-005
    #[test]
    fn tran_cat_balance_records_round_trip(
        record in tran_cat_balance_strategy(),
        terminator in terminator(),
        cut in 0..=TranCatBalanceRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    // audit: CVTRA02Y-001
    // audit: CVTRA02Y-002
    // audit: CVTRA02Y-003
    // audit: CVTRA02Y-004
    // audit: CVTRA02Y-005
    #[test]
    fn disclosure_group_records_round_trip(
        record in disclosure_group_strategy(),
        terminator in terminator(),
        cut in 0..=DisclosureGroupRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    // audit: CVTRA03Y-001
    // audit: CVTRA03Y-002
    // audit: CVTRA03Y-003
    #[test]
    fn transaction_type_records_round_trip(
        record in transaction_type_strategy(),
        terminator in terminator(),
        cut in 0..=TransactionTypeRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    // audit: CVTRA04Y-001
    // audit: CVTRA04Y-002
    // audit: CVTRA04Y-003
    // audit: CVTRA04Y-004
    #[test]
    fn transaction_category_records_round_trip(
        record in transaction_category_strategy(),
        terminator in terminator(),
        cut in 0..=TransactionCategoryRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    // audit: CSUSR01Y-001
    // audit: CSUSR01Y-002
    // audit: CSUSR01Y-003
    // audit: CSUSR01Y-004
    // audit: CSUSR01Y-005
    // audit: CSUSR01Y-006
    // audit: PIC-012
    // audit: PIC-013
    // audit: PIC-014
    #[test]
    fn user_security_records_round_trip(
        record in user_security_strategy(),
        terminator in terminator(),
        cut in 0..=UserSecurityRecord::TRAILING_FILLER,
    ) {
        record_round_trip(&record)?;
        framed_round_trip(&record, terminator, cut)?;
    }

    // audit: CIPAUSMY-001
    // audit: CIPAUSMY-002
    // audit: CIPAUSMY-003
    // audit: CIPAUSMY-004
    // audit: CIPAUSMY-005
    // audit: CIPAUSMY-006
    // audit: CIPAUSMY-007
    // audit: CIPAUSMY-008
    // audit: CIPAUSMY-009
    // audit: CIPAUSMY-010
    // audit: CIPAUSMY-011
    // audit: CIPAUSMY-012
    // audit: CIPAUSMY-013
    // audit: PIC-010
    // audit: PIC-011
    #[test]
    fn pending_auth_summary_records_round_trip(record in pending_auth_summary_strategy()) {
        record_round_trip(&record)?;
    }

    // audit: CIPAUDTY-001
    // audit: CIPAUDTY-002
    // audit: CIPAUDTY-003
    // audit: CIPAUDTY-004
    // audit: CIPAUDTY-005
    // audit: CIPAUDTY-006
    // audit: CIPAUDTY-007
    // audit: CIPAUDTY-008
    // audit: CIPAUDTY-009
    // audit: CIPAUDTY-010
    // audit: CIPAUDTY-011
    // audit: CIPAUDTY-012
    // audit: CIPAUDTY-013
    // audit: CIPAUDTY-014
    // audit: CIPAUDTY-015
    // audit: CIPAUDTY-016
    // audit: CIPAUDTY-017
    // audit: CIPAUDTY-018
    // audit: CIPAUDTY-019
    // audit: CIPAUDTY-020
    // audit: CIPAUDTY-021
    // audit: CIPAUDTY-022
    // audit: CIPAUDTY-023
    // audit: CIPAUDTY-024
    // audit: CIPAUDTY-025
    // audit: CIPAUDTY-026
    // audit: CIPAUDTY-027
    // audit: CIPAUDTY-028
    #[test]
    fn pending_auth_detail_records_round_trip(record in pending_auth_detail_strategy()) {
        record_round_trip(&record)?;
    }

    // audit: CCPAURQY-001
    // audit: CCPAURQY-002
    // audit: CCPAURQY-003
    // audit: CCPAURQY-004
    // audit: CCPAURQY-005
    // audit: CCPAURQY-006
    // audit: CCPAURQY-007
    // audit: CCPAURQY-008
    // audit: CCPAURQY-009
    // audit: CCPAURQY-010
    // audit: CCPAURQY-011
    // audit: CCPAURQY-012
    // audit: CCPAURQY-013
    // audit: CCPAURQY-014
    // audit: CCPAURQY-015
    // audit: CCPAURQY-016
    // audit: CCPAURQY-017
    // audit: CCPAURQY-018
    // audit: PIC-022
    #[test]
    fn auth_request_records_round_trip(record in auth_request_strategy()) {
        record_round_trip(&record)?;
    }

    // audit: CCPAURLY-001
    // audit: CCPAURLY-002
    // audit: CCPAURLY-003
    // audit: CCPAURLY-004
    // audit: CCPAURLY-005
    // audit: CCPAURLY-006
    #[test]
    fn auth_reply_records_round_trip(record in auth_reply_strategy()) {
        record_round_trip(&record)?;
    }

    // audit: CCPAUERY-001
    // audit: CCPAUERY-002
    // audit: CCPAUERY-003
    // audit: CCPAUERY-004
    // audit: CCPAUERY-005
    // audit: CCPAUERY-006
    // audit: CCPAUERY-007
    // audit: CCPAUERY-008
    // audit: CCPAUERY-009
    // audit: CCPAUERY-010
    // audit: CCPAUERY-011
    #[test]
    fn error_log_records_round_trip(record in error_log_strategy()) {
        record_round_trip(&record)?;
    }
}
