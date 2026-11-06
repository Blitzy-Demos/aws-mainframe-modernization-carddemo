//! The ASCII and EBCDIC fixture corpora hold the same content except at the two
//! fields of escalation E-05 (AAP 0.5.3.2 "Tests", goal o4: "the cross-check
//! differs only at E-05").
//!
//! Every record of the nine counterpart pairs, `app/data/ASCII/*.txt` and
//! `app/data/EBCDIC/AWS.M2.CARDDEMO.*.PS`, is decoded in its own character set,
//! re-expressed as internal bytes through its layout, and compared field by
//! field, FILLER included. Exactly two fields differ:
//!
//! - `acctdata.txt` record 49, `ACCT-ADDR-ZIP` (`CVACT01Y` offset 102, length
//!   10): `A000000000` in ASCII, `ZEROAPR` followed by three spaces in EBCDIC.
//! - `discgrp.txt` record 34 (`DEFAULT`, `07`, `0001`), `DIS-INT-RATE`
//!   (`CVTRA02Y` offset 16, length 6): `00000{` in ASCII, `00150{` in EBCDIC.
//!
//! Any other difference, or either of these two vanishing, is a failure that
//! needs a new escalation in `docs/parity/escalations.md`. Neither corpus is
//! repaired ("PIC clauses are authoritative"): ASCII seeds the database and
//! drives parity, and EBCDIC serves only the o4 round trip.
//!
//! ASCII records are padded with spaces to their copybook length before they
//! are decoded, so the 36-byte `cardxref.txt` records (E-04) compare at 50
//! bytes, their absent FILLER matching the `0x40` (EBCDIC space) filler.

use carddemo_codec::{
    AccountLayout, AccountRecord, CardLayout, CardXrefLayout, Charset, CustomerLayout,
    DailyTranLayout, DisclosureGroupLayout, DisclosureGroupRecord, Layout, TranCatBalanceLayout,
    TranCategoryLayout, TranTypeLayout, pad, split_ascii,
};
use carddemo_domain::entities::DEFAULT_GROUP_ID;

/// The ASCII fixture corpus, the parity oracle input.
const ASCII_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/data/ASCII");

/// The EBCDIC (CP037) counterpart corpus.
const EBCDIC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/data/EBCDIC");

/// The parity audit, whose section E row [`RECORD_34_ROW`] states the rates of
/// disclosure group record 34 in each corpus.
const PARITY_AUDIT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/parity/parity-oracle-audit.md"
);

/// The audit row of the E-05 rate difference, as corrected by E-79.
const RECORD_34_ROW: &str = "P-CVTRA02Y-007";

/// The failure message of a cross-check whose differences are not exactly
/// E-05's.
const NOT_E05: &str = "ASCII/EBCDIC content differs outside E-05 (or an E-05 difference \
                       vanished): log a new escalation in docs/parity/escalations.md";

/// One field whose content differs between an ASCII record and its EBCDIC
/// counterpart.
#[derive(Debug, PartialEq, Eq)]
struct Difference {
    /// The ASCII fixture file.
    file: &'static str,
    /// The 1-based record number, the same in both files.
    record: usize,
    /// The copybook field name.
    field: &'static str,
    /// The field's internal bytes in the ASCII record, one `char` per byte
    /// (Latin-1), so trailing spaces stay visible.
    ascii: String,
    /// The field's internal bytes in the EBCDIC record, after CP037
    /// translation, one `char` per byte.
    ebcdic: String,
}

/// One record of a file pair, decoded from each corpus.
struct RecordPair<R> {
    /// The ASCII record, decoded after padding to the layout length.
    ascii: R,
    /// The EBCDIC record, decoded from its fixed-length block.
    ebcdic: R,
    /// The EBCDIC block translated byte for byte through the CP037 table.
    ebcdic_internal: Vec<u8>,
}

/// Reads the fixture `file` of the corpus directory `dir`.
fn read_fixture(dir: &str, file: &str) -> Vec<u8> {
    let path = format!("{dir}/{file}");
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read fixture {path}: {error}"))
}

/// The bytes as text, one `char` per byte (Latin-1).
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

/// Decodes every record of `ascii_file` and of its EBCDIC counterpart
/// `ebcdic_file` with the layout `L`, paired by record number.
///
/// Panics, naming the file and the 1-based record number, when a file cannot
/// be read, the EBCDIC file is not a whole number of `L::LEN`-byte blocks, the
/// two files hold different record counts, or a record fails to pad or decode.
fn decode_pair<L: Layout>(ascii_file: &str, ebcdic_file: &str) -> Vec<RecordPair<L::Record>> {
    let ascii_bytes = read_fixture(ASCII_DIR, ascii_file);
    let ebcdic_bytes = read_fixture(EBCDIC_DIR, ebcdic_file);
    let ascii_records = split_ascii(&ascii_bytes);
    let blocks = ebcdic_bytes.chunks_exact(L::LEN);
    assert!(
        blocks.remainder().is_empty(),
        "{ebcdic_file}: {} bytes is not a whole number of {}-byte {} records",
        ebcdic_bytes.len(),
        L::LEN,
        L::NAME
    );
    assert_eq!(
        ascii_records.len(),
        blocks.len(),
        "{ascii_file} and {ebcdic_file} hold different record counts"
    );
    ascii_records
        .into_iter()
        .zip(blocks)
        .zip(1_usize..)
        .map(|(((_, body), block), record)| {
            let padded = pad(L::NAME, body, L::LEN)
                .unwrap_or_else(|error| panic!("{ascii_file} record {record}: {error}"));
            RecordPair {
                ascii: L::decode(&padded, Charset::Ascii)
                    .unwrap_or_else(|error| panic!("{ascii_file} record {record}: {error}")),
                ebcdic: L::decode(block, Charset::Ebcdic)
                    .unwrap_or_else(|error| panic!("{ebcdic_file} record {record}: {error}")),
                ebcdic_internal: Charset::Ebcdic.to_internal(block),
            }
        })
        .collect()
}

/// The fields whose content differs between `ascii_file` and its EBCDIC
/// counterpart `ebcdic_file`, both read with the layout `L`, in record order
/// and then copybook field order.
///
/// Both decoded records are re-expressed as internal bytes by encoding them in
/// ASCII, so the two sides are compared through the same layout. The EBCDIC
/// side must also equal the raw CP037 translation of its block, which proves
/// that the layout path and the character-set table agree.
fn crosscheck<L: Layout>(ascii_file: &'static str, ebcdic_file: &str) -> Vec<Difference> {
    let mut differences = Vec::new();
    for (pair, record) in decode_pair::<L>(ascii_file, ebcdic_file)
        .iter()
        .zip(1_usize..)
    {
        let ascii = L::encode(&pair.ascii, Charset::Ascii);
        let ebcdic = L::encode(&pair.ebcdic, Charset::Ascii);
        assert_eq!(
            ebcdic, pair.ebcdic_internal,
            "{ebcdic_file} record {record}: the layout's internal bytes differ from the CP037 \
             translation of the block"
        );
        for spec in L::FIELDS {
            let window = spec.offset..spec.end();
            let (ascii_field, ebcdic_field) = (&ascii[window.clone()], &ebcdic[window]);
            if ascii_field != ebcdic_field {
                differences.push(Difference {
                    file: ascii_file,
                    record,
                    field: spec.name,
                    ascii: latin1(ascii_field),
                    ebcdic: latin1(ebcdic_field),
                });
            }
        }
    }
    differences
}

/// The cells of the data row `id` of [`PARITY_AUDIT`]: the table line whose
/// first cell is `id` and whose `Program` cell is the key of `id`, which
/// passes over the review-log lines that also start with `id`.
fn audit_cells(id: &str) -> Vec<String> {
    let text = std::fs::read_to_string(PARITY_AUDIT)
        .unwrap_or_else(|error| panic!("cannot read {PARITY_AUDIT}: {error}"));
    let key = id
        .strip_prefix("P-")
        .and_then(|rest| rest.rsplit_once('-'))
        .map_or_else(|| panic!("{id} is not a P-ID"), |(key, _)| key);
    text.lines()
        .map(|line| {
            line.trim()
                .trim_matches('|')
                .split(" | ")
                .map(|cell| cell.trim().to_owned())
                .collect::<Vec<String>>()
        })
        .find(|cells| {
            cells.first().is_some_and(|first| first == id)
                && cells.get(1).is_some_and(|program| program == key)
        })
        .unwrap_or_else(|| panic!("{PARITY_AUDIT} has no data row {id}"))
}

/// Cell `index` of the audit row `id`, whose cells are `cells`.
fn cell<'c>(cells: &'c [String], index: usize, id: &str) -> &'c str {
    cells
        .get(index)
        .map_or_else(|| panic!("{id} has no cell {index}"), String::as_str)
}

/// The exact texts of an audit cell: the double-quoted string of each code
/// span.
fn exact_texts(cell: &str) -> Vec<&str> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .map(|span| span.trim_matches('"'))
        .collect()
}

/// The rate written after `label` in `text`, in hundredths: `15.00` is 1 500.
fn rate_after(text: &str, label: &str) -> i32 {
    let rate = text
        .split_once(label)
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .map_or_else(|| panic!("{text:?} gives no {label:?}"), |word| word);
    let (whole, cents) = rate
        .split_once('.')
        .filter(|(_, cents)| cents.len() == 2)
        .unwrap_or_else(|| panic!("{rate:?} is not a rate with two decimals"));
    let number = |digits: &str| -> i32 {
        digits
            .parse()
            .unwrap_or_else(|error| panic!("{rate:?}: {error}"))
    };
    number(whole) * 100 + number(cents)
}

/// Record `number` (1-based) of a decoded file pair.
fn record<R>(pairs: &[RecordPair<R>], number: usize) -> &RecordPair<R> {
    number
        .checked_sub(1)
        .and_then(|index| pairs.get(index))
        .unwrap_or_else(|| panic!("no record {number} among {} records", pairs.len()))
}

#[test]
fn corpora_differ_only_at_e05() {
    let differences: Vec<Difference> = [
        crosscheck::<AccountLayout>("acctdata.txt", "AWS.M2.CARDDEMO.ACCTDATA.PS"),
        crosscheck::<CardLayout>("carddata.txt", "AWS.M2.CARDDEMO.CARDDATA.PS"),
        crosscheck::<CardXrefLayout>("cardxref.txt", "AWS.M2.CARDDEMO.CARDXREF.PS"),
        crosscheck::<CustomerLayout>("custdata.txt", "AWS.M2.CARDDEMO.CUSTDATA.PS"),
        crosscheck::<DailyTranLayout>("dailytran.txt", "AWS.M2.CARDDEMO.DALYTRAN.PS"),
        crosscheck::<DisclosureGroupLayout>("discgrp.txt", "AWS.M2.CARDDEMO.DISCGRP.PS"),
        crosscheck::<TranCatBalanceLayout>("tcatbal.txt", "AWS.M2.CARDDEMO.TCATBALF.PS"),
        crosscheck::<TranCategoryLayout>("trancatg.txt", "AWS.M2.CARDDEMO.TRANCATG.PS"),
        crosscheck::<TranTypeLayout>("trantype.txt", "AWS.M2.CARDDEMO.TRANTYPE.PS"),
    ]
    .into_iter()
    .flatten()
    .collect();

    let expected = vec![
        Difference {
            file: "acctdata.txt",
            record: 49,
            field: "ACCT-ADDR-ZIP",
            ascii: String::from("A000000000"),
            ebcdic: String::from("ZEROAPR   "),
        },
        Difference {
            file: "discgrp.txt",
            record: 34,
            field: "DIS-INT-RATE",
            ascii: String::from("00000{"),
            ebcdic: String::from("00150{"),
        },
    ];
    assert_eq!(differences, expected, "{NOT_E05}");
    insta::assert_debug_snapshot!("e05_differences", differences);
}

#[test]
fn account_record_49_differs_only_in_its_zip() {
    let pairs: Vec<RecordPair<AccountRecord>> =
        decode_pair::<AccountLayout>("acctdata.txt", "AWS.M2.CARDDEMO.ACCTDATA.PS");
    let RecordPair { ascii, ebcdic, .. } = record(&pairs, 49);

    assert_eq!(ascii.account.acct_addr_zip.as_bytes(), b"A000000000");
    assert_eq!(ebcdic.account.acct_addr_zip.as_bytes(), b"ZEROAPR   ");
    for (corpus, side) in [("ASCII", ascii), ("EBCDIC", ebcdic)] {
        assert_eq!(side.account.acct_id.get(), 49, "{corpus} ACCT-ID");
        // E-06: the group ID is blank in both corpora, so neither ZIP value is
        // a misplaced group ID.
        assert!(
            side.account.acct_group_id.as_alnum().is_blank(),
            "{corpus} ACCT-GROUP-ID is not blank"
        );
    }

    let mut aligned = ebcdic.clone();
    aligned.account.acct_addr_zip = ascii.account.acct_addr_zip;
    assert_eq!(&aligned, ascii, "{NOT_E05}");
}

/// [`RECORD_34_ROW`] read from the audit: the record, key and field it names
/// hold, in each corpus, the field bytes and the rate the row gives, and
/// nothing else in the record differs.
#[test]
fn disclosure_group_record_34_differs_only_in_its_rate() {
    let id = RECORD_34_ROW;
    let cells = audit_cells(id);
    let (number, row_key) = cell(&cells, 4, id)
        .strip_prefix("record ")
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|rest| rest.split_once(" ("))
        .unwrap_or_else(|| panic!("{id}: the input condition names no record and key"));
    let number: usize = number
        .parse()
        .unwrap_or_else(|error| panic!("{id}: record {number:?}: {error}"));
    let row_key: Vec<&str> = row_key.split(", ").collect();
    let field_name = cell(&cells, 3, id)
        .split_whitespace()
        .next()
        .unwrap_or_else(|| panic!("{id} names no field"));
    let spec = DisclosureGroupLayout::FIELDS
        .iter()
        .find(|spec| spec.name == field_name)
        .unwrap_or_else(|| panic!("{id}: DIS-GROUP-RECORD has no {field_name}"));
    let disposition = cell(&cells, 5, id);
    let texts = exact_texts(disposition);
    let [ascii_bytes, ebcdic_bytes] = texts.as_slice() else {
        panic!("{id}: {disposition:?} gives no ASCII and EBCDIC field bytes")
    };

    let pairs: Vec<RecordPair<DisclosureGroupRecord>> =
        decode_pair::<DisclosureGroupLayout>("discgrp.txt", "AWS.M2.CARDDEMO.DISCGRP.PS");
    let RecordPair {
        ascii,
        ebcdic,
        ebcdic_internal,
    } = record(&pairs, number);

    for (corpus, side) in [("ASCII", ascii), ("EBCDIC", ebcdic)] {
        let key = &side.group.key;
        assert_eq!(
            key.dis_acct_group_id.as_bytes(),
            &DEFAULT_GROUP_ID,
            "{corpus} DIS-ACCT-GROUP-ID"
        );
        let held = [
            key.dis_acct_group_id.trimmed().into_owned(),
            key.dis_tran_type_cd.trimmed().into_owned(),
            key.dis_tran_cat_cd.to_string(),
        ];
        assert_eq!(held.as_slice(), row_key.as_slice(), "{id}: {corpus} key");
    }

    // The field's bytes as the row gives them, then the rates `S9(04)V99`
    // reads from them: the implied point falls before the last two digits.
    let window = spec.offset..spec.end();
    let fields = (
        latin1(&DisclosureGroupLayout::encode(ascii, Charset::Ascii)[window.clone()]),
        latin1(&ebcdic_internal[window]),
    );
    assert_eq!(
        (fields.0.as_str(), fields.1.as_str()),
        (*ascii_bytes, *ebcdic_bytes),
        "{id}: {field_name} bytes in ASCII and after CP037 translation"
    );
    assert_eq!(
        ascii.group.dis_int_rate.hundredths(),
        rate_after(disposition, "ASCII rate "),
        "{id}: ASCII rate"
    );
    assert_eq!(
        ebcdic.group.dis_int_rate.hundredths(),
        rate_after(disposition, "EBCDIC rate "),
        "{id}: EBCDIC rate"
    );

    let mut aligned = ebcdic.clone();
    aligned.group.dis_int_rate = ascii.group.dis_int_rate;
    assert_eq!(&aligned, ascii, "{NOT_E05}");
}
