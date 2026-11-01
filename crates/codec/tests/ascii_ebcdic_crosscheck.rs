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

#[test]
fn disclosure_group_record_34_differs_only_in_its_rate() {
    let pairs: Vec<RecordPair<DisclosureGroupRecord>> =
        decode_pair::<DisclosureGroupLayout>("discgrp.txt", "AWS.M2.CARDDEMO.DISCGRP.PS");
    let RecordPair { ascii, ebcdic, .. } = record(&pairs, 34);

    for (corpus, side) in [("ASCII", ascii), ("EBCDIC", ebcdic)] {
        let key = &side.group.key;
        assert_eq!(
            key.dis_acct_group_id.as_bytes(),
            &DEFAULT_GROUP_ID,
            "{corpus} DIS-ACCT-GROUP-ID"
        );
        assert_eq!(
            key.dis_tran_type_cd.as_bytes(),
            b"07",
            "{corpus} DIS-TRAN-TYPE-CD"
        );
        assert_eq!(key.dis_tran_cat_cd.get(), 1, "{corpus} DIS-TRAN-CAT-CD");
    }

    assert_eq!(ascii.group.dis_int_rate.hundredths(), 0, "ASCII 0.00");
    // `S9(04)V99` puts the implied point before the last two of the digits
    // `001500`, so the EBCDIC rate is 15.00, as record 1's identical `00150{`
    // is. E-05 and P-CVTRA02Y-007 state it as 1.50, which these bytes do not
    // spell; the PIC clause decides ("PIC clauses are authoritative").
    assert_eq!(
        ebcdic.group.dis_int_rate.hundredths(),
        1_500,
        "EBCDIC 15.00"
    );

    let mut aligned = ebcdic.clone();
    aligned.group.dis_int_rate = ascii.group.dis_int_rate;
    assert_eq!(&aligned, ascii, "{NOT_E05}");
}
