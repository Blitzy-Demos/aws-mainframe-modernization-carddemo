//! Goal o4 over the fixture corpus (AAP 0.5.3.2 "Tests", 0.10.1 row o4): every
//! record and every file of `app/data/ASCII` and of its EBCDIC counterpart in
//! `app/data/EBCDIC` decodes through its copybook layout and re-encodes to its
//! stored bytes, byte for byte.
//!
//! - ASCII: the 626 records of the nine `*.txt` files, at record level (each
//!   stored body padded to the layout length) and at file level (each frame
//!   re-applied as stored), together with the E-04 framing: `CR LF` line ends
//!   in `trancatg.txt`, `tcatbal.txt` and `trantype.txt`, and the records of
//!   `cardxref.txt` stored as the first 36 of their 50 bytes.
//! - EBCDIC: the 626 records of the nine `AWS.M2.CARDDEMO.*.PS` counterparts,
//!   fixed-length CP037 blocks with no line ends.
//! - Outside the 626: `AWS.M2.CARDDEMO.ACCDATA.PS`, a byte copy of
//!   `AWS.M2.CARDDEMO.ACCTDATA.PS`; `AWS.M2.CARDDEMO.DALYTRAN.PS.INIT`, the
//!   low-values `TRANFILE` record (E-07); `AWS.M2.CARDDEMO.EXPORT.DATA.PS`, the
//!   `CBEXPORT` stream, which no layout in scope encodes; and
//!   `AWS.M2.CARDDEMO.USRSEC.PS`, the ten `CSUSR01Y` user rows, round-tripped
//!   here without reading their passwords. `TransactionLayout` (`CVTRA05Y`) has
//!   no fixture of its own and is also run over the `CVTRA06Y` daily
//!   transactions, whose field widths it shares; those runs are extra coverage,
//!   not part of the 626.
//!
//! The fixtures are the parity oracle: they are only read, never written and
//! never repaired ("PIC clauses are authoritative"). A decoded fixture record
//! that keeps preserved bytes (a negative-zero overpunch) is an escalation for
//! `docs/parity/escalations.md`, never a fix.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Display};
use std::path::Path;

use carddemo_codec::{
    AccountLayout, AccountRecord, AsciiFrame, CardLayout, CardXrefLayout, Charset, CustomerLayout,
    DailyTranLayout, DailyTranRecord, DisclosureGroupLayout, DisclosureGroupRecord, Framed, Layout,
    PreservedFields, Terminator, TranCatBalanceLayout, TranCatBalanceRecord, TranCategoryLayout,
    TranTypeLayout, TransactionLayout, TransactionRecord, UserSecurityLayout, decode_ascii_file,
    decode_ebcdic_file, encode_ascii_file, encode_ebcdic_file, pad, split_ascii,
};

/// The ASCII fixture corpus, read only.
const ASCII_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/data/ASCII");

/// The EBCDIC fixture corpus, read only.
const EBCDIC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/data/EBCDIC");

/// Every ASCII fixture file with its record count: 626 records in all.
const ASCII_COUNTS: [(&str, usize); 9] = [
    ("acctdata.txt", 50),
    ("carddata.txt", 50),
    ("cardxref.txt", 50),
    ("custdata.txt", 50),
    ("dailytran.txt", 300),
    ("discgrp.txt", 51),
    ("tcatbal.txt", 50),
    ("trancatg.txt", 18),
    ("trantype.txt", 7),
];

/// The record count of the whole ASCII corpus, and of its EBCDIC counterpart.
const CORPUS_RECORDS: usize = 626;

/// The stored body length of every `cardxref.txt` record: `XREF-CARD-NUM`,
/// `XREF-CUST-ID` and `XREF-ACCT-ID`, without the 14-byte `CVACT03Y` FILLER
/// (E-04).
const CARDXREF_STORED_LEN: usize = 36;

/// What the absent `cardxref.txt` FILLER decodes to, and what the `0x40`
/// filler of `AWS.M2.CARDDEMO.CARDXREF.PS` decodes to: 14 spaces.
const XREF_FILLER: [u8; 14] = [b' '; 14];

/// One EBCDIC counterpart file: the ASCII file it mirrors record for record,
/// its layout's record length and its size in bytes.
struct Counterpart {
    ebcdic: &'static str,
    ascii: &'static str,
    record_len: usize,
    file_bytes: usize,
}

/// The nine EBCDIC counterparts of the ASCII corpus: 626 records in all.
const EBCDIC_COUNTERPARTS: [Counterpart; 9] = [
    Counterpart {
        ebcdic: "AWS.M2.CARDDEMO.ACCTDATA.PS",
        ascii: "acctdata.txt",
        record_len: AccountLayout::LEN,
        file_bytes: 15_000,
    },
    Counterpart {
        ebcdic: "AWS.M2.CARDDEMO.CARDDATA.PS",
        ascii: "carddata.txt",
        record_len: CardLayout::LEN,
        file_bytes: 7500,
    },
    Counterpart {
        ebcdic: "AWS.M2.CARDDEMO.CARDXREF.PS",
        ascii: "cardxref.txt",
        record_len: CardXrefLayout::LEN,
        file_bytes: 2500,
    },
    Counterpart {
        ebcdic: "AWS.M2.CARDDEMO.CUSTDATA.PS",
        ascii: "custdata.txt",
        record_len: CustomerLayout::LEN,
        file_bytes: 25_000,
    },
    Counterpart {
        ebcdic: "AWS.M2.CARDDEMO.DALYTRAN.PS",
        ascii: "dailytran.txt",
        record_len: DailyTranLayout::LEN,
        file_bytes: 105_000,
    },
    Counterpart {
        ebcdic: "AWS.M2.CARDDEMO.DISCGRP.PS",
        ascii: "discgrp.txt",
        record_len: DisclosureGroupLayout::LEN,
        file_bytes: 2550,
    },
    Counterpart {
        ebcdic: "AWS.M2.CARDDEMO.TCATBALF.PS",
        ascii: "tcatbal.txt",
        record_len: TranCatBalanceLayout::LEN,
        file_bytes: 2500,
    },
    Counterpart {
        ebcdic: "AWS.M2.CARDDEMO.TRANCATG.PS",
        ascii: "trancatg.txt",
        record_len: TranCategoryLayout::LEN,
        file_bytes: 1080,
    },
    Counterpart {
        ebcdic: "AWS.M2.CARDDEMO.TRANTYPE.PS",
        ascii: "trantype.txt",
        record_len: TranTypeLayout::LEN,
        file_bytes: 420,
    },
];

/// The EBCDIC files outside the 626 counterpart records.
const EBCDIC_EXCLUDED: [&str; 4] = [
    "AWS.M2.CARDDEMO.ACCDATA.PS",
    "AWS.M2.CARDDEMO.DALYTRAN.PS.INIT",
    "AWS.M2.CARDDEMO.EXPORT.DATA.PS",
    "AWS.M2.CARDDEMO.USRSEC.PS",
];

/// The placeholder that keeps the EBCDIC directory in git.
const GITKEEP: &str = ".gitkeep";

/// `SEC-USR-ID` and `SEC-USR-TYPE` of the ten `USRSEC` rows, in file order.
const USERS: [(&str, &str); 10] = [
    ("ADMIN001", "A"),
    ("ADMIN002", "A"),
    ("ADMIN003", "A"),
    ("ADMIN004", "A"),
    ("ADMIN005", "A"),
    ("USER0001", "U"),
    ("USER0002", "U"),
    ("USER0003", "U"),
    ("USER0004", "U"),
    ("USER0005", "U"),
];

/// The bytes of the fixture `name` in `dir`.
fn read_fixture(dir: &str, name: &str) -> Vec<u8> {
    let path = Path::new(dir).join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The bytes of the ASCII fixture `name`.
fn read_ascii(name: &str) -> Vec<u8> {
    read_fixture(ASCII_DIR, name)
}

/// The bytes of the EBCDIC fixture `name`.
fn read_ebcdic(name: &str) -> Vec<u8> {
    read_fixture(EBCDIC_DIR, name)
}

/// The record count [`ASCII_COUNTS`] gives the ASCII file `name`.
fn ascii_count(name: &str) -> usize {
    ASCII_COUNTS
        .iter()
        .find_map(|&(file, count)| (file == name).then_some(count))
        .unwrap_or_else(|| panic!("{name} is not an ASCII fixture file"))
}

/// The offset of the first byte at which `a` and `b` differ, or the shorter
/// length when one is a prefix of the other. Failure messages give this offset
/// instead of both buffers, which run to 105 kB.
fn first_difference(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or_else(|| a.len().min(b.len()))
}

/// Round-trips the ASCII fixture `file` through the layout `L` and returns its
/// decoded records with their frames.
///
/// Record level: each stored body, padded with spaces to `L::LEN`, decodes and
/// re-encodes to the padded bytes. File level: the decoded file re-encodes to
/// the stored file, every frame re-applied.
fn roundtrip_ascii<L: Layout>(file: &str, expected_count: usize) -> Vec<Framed<L::Record>> {
    let bytes = read_ascii(file);
    let split = split_ascii(&bytes);
    assert_eq!(split.len(), expected_count, "{file}: record count");
    for (&(_, body), n) in split.iter().zip(1_usize..) {
        let padded =
            pad(L::NAME, body, L::LEN).unwrap_or_else(|e| panic!("{file} record {n}: {e}"));
        let record =
            L::decode(&padded, Charset::Ascii).unwrap_or_else(|e| panic!("{file} record {n}: {e}"));
        let encoded = L::encode(&record, Charset::Ascii);
        assert!(
            encoded == padded,
            "{file} record {n}: the re-encoded record differs from the stored one at offset {}",
            first_difference(&encoded, &padded)
        );
    }
    let decoded = decode_ascii_file::<L>(&bytes).unwrap_or_else(|e| panic!("{file}: {e}"));
    assert_eq!(
        decoded.len(),
        expected_count,
        "{file}: decoded record count"
    );
    let encoded = encode_ascii_file::<L>(&decoded).unwrap_or_else(|e| panic!("{file}: {e}"));
    assert!(
        encoded == bytes,
        "{file}: the re-encoded file ({} bytes) differs from the fixture ({} bytes) at offset {}",
        encoded.len(),
        bytes.len(),
        first_difference(&encoded, &bytes)
    );
    decoded
}

/// Round-trips the EBCDIC fixture `file` through the layout `L` and returns its
/// decoded records.
///
/// The file must be exactly `expected_count` blocks of `L::LEN` bytes, the
/// record count of its ASCII counterpart. Record level: each block decodes and
/// re-encodes to itself. File level: the decoded file re-encodes to the stored
/// file.
fn roundtrip_ebcdic<L: Layout>(file: &str, expected_count: usize) -> Vec<L::Record> {
    let bytes = read_ebcdic(file);
    assert!(
        bytes.len().is_multiple_of(L::LEN),
        "{file}: {} bytes is not a whole number of {}-byte {} records",
        bytes.len(),
        L::LEN,
        L::NAME
    );
    assert_eq!(bytes.len() / L::LEN, expected_count, "{file}: record count");
    for (block, n) in bytes.chunks_exact(L::LEN).zip(1_usize..) {
        let record =
            L::decode(block, Charset::Ebcdic).unwrap_or_else(|e| panic!("{file} record {n}: {e}"));
        let encoded = L::encode(&record, Charset::Ebcdic);
        assert!(
            encoded == block,
            "{file} record {n}: the re-encoded record differs from the stored one at offset {}",
            first_difference(&encoded, block)
        );
    }
    let decoded = decode_ebcdic_file::<L>(&bytes).unwrap_or_else(|e| panic!("{file}: {e}"));
    assert_eq!(
        decoded.len(),
        expected_count,
        "{file}: decoded record count"
    );
    let encoded = encode_ebcdic_file::<L>(&decoded);
    assert!(
        encoded == bytes,
        "{file}: the re-encoded file ({} bytes) differs from the fixture ({} bytes) at offset {}",
        encoded.len(),
        bytes.len(),
        first_difference(&encoded, &bytes)
    );
    decoded
}

/// A record with signed `S9(n)V99` fields, whose decoder keeps the stored bytes
/// of any value its typed field cannot reproduce.
trait Preserving {
    fn preserved(&self) -> &PreservedFields;
}

impl Preserving for AccountRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

impl Preserving for DailyTranRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

impl Preserving for TransactionRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

impl Preserving for TranCatBalanceRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

impl Preserving for DisclosureGroupRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

/// Asserts that no record of `file` kept preserved bytes: every signed value
/// in the corpus round-trips through its typed field alone.
fn assert_nothing_preserved<'a, R: Preserving + 'a>(
    file: &str,
    records: impl IntoIterator<Item = &'a R>,
) {
    for (record, n) in records.into_iter().zip(1_usize..) {
        let preserved = record.preserved();
        assert!(
            preserved.is_empty(),
            "{file} record {n}: preserved fields {:?}: negative-zero overpunch found in the \
             fixture corpus: log a new escalation in docs/parity/escalations.md",
            preserved.iter().map(|raw| raw.field).collect::<Vec<_>>()
        );
    }
}

/// Asserts the frame of every record of `file`: the stored length and line end
/// `expected` gives for its 1-based record number.
fn assert_frames<R>(file: &str, records: &[Framed<R>], expected: impl Fn(usize) -> AsciiFrame) {
    for (framed, n) in records.iter().zip(1_usize..) {
        assert_eq!(framed.frame, expected(n), "{file} record {n}: frame");
    }
}

/// The snapshot label of a line end.
fn terminator_label(terminator: Terminator) -> &'static str {
    match terminator {
        Terminator::Lf => "LF",
        Terminator::CrLf => "CRLF",
        Terminator::None => "none",
    }
}

/// `items` as runs of equal values in order, for example `CRLF x49, LF x1`.
fn run_lengths<T: PartialEq + Display>(items: impl IntoIterator<Item = T>) -> String {
    let mut runs: Vec<(T, usize)> = Vec::new();
    for item in items {
        match runs.last_mut() {
            Some((last, count)) if *last == item => *count += 1,
            _ => runs.push((item, 1)),
        }
    }
    runs.iter()
        .map(|(item, count)| format!("{item} x{count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The framing of a decoded ASCII file: its record count, its size, and the
/// runs of its line ends and stored lengths.
fn frame_summary<R>(records: &[Framed<R>], file_bytes: usize) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("count".to_owned(), records.len().to_string()),
        ("file_bytes".to_owned(), file_bytes.to_string()),
        (
            "terminators".to_owned(),
            run_lengths(
                records
                    .iter()
                    .map(|framed| terminator_label(framed.frame.terminator)),
            ),
        ),
        (
            "stored_lengths".to_owned(),
            run_lengths(records.iter().map(|framed| framed.frame.stored_len)),
        ),
    ])
}

/// The full check of the ASCII fixture `<stem>.txt` through the layout `L`:
/// [`roundtrip_ascii`] with its [`ASCII_COUNTS`] entry, its size, the frame of
/// every record and no missing line end, then the snapshots
/// `<stem>_first_last` (the first and last decoded records) and
/// `<stem>_frames` (the [`frame_summary`]).
fn check_ascii<L: Layout>(
    stem: &str,
    file_bytes: usize,
    expected_frame: impl Fn(usize) -> AsciiFrame,
) -> Vec<Framed<L::Record>>
where
    L::Record: Debug,
{
    let file = format!("{stem}.txt");
    let records = roundtrip_ascii::<L>(&file, ascii_count(&file));
    assert_eq!(read_ascii(&file).len(), file_bytes, "{file}: file size");
    assert_frames(&file, &records, expected_frame);
    assert!(
        records
            .iter()
            .all(|framed| framed.frame.terminator != Terminator::None),
        "{file}: a record has no line end"
    );
    let (Some(first), Some(last)) = (records.first(), records.last()) else {
        panic!("{file}: no records");
    };
    insta::assert_debug_snapshot!(format!("{stem}_first_last"), (&first.record, &last.record));
    insta::assert_yaml_snapshot!(
        format!("{stem}_frames"),
        frame_summary(&records, file_bytes)
    );
    records
}

/// The frame of a record stored as `stored_len` bytes ending `CR LF` while its
/// 1-based number `n` is below `last`, and ending `LF` at `last`: the E-04
/// framing of `tcatbal.txt` and `trantype.txt`.
fn crlf_until_last(n: usize, last: usize, stored_len: usize) -> AsciiFrame {
    AsciiFrame {
        stored_len,
        terminator: if n < last {
            Terminator::CrLf
        } else {
            Terminator::Lf
        },
    }
}

/// The full check of the EBCDIC counterpart `ebcdic` through the layout `L`:
/// its [`EBCDIC_COUNTERPARTS`] entry names the same record length and file
/// size, and [`roundtrip_ebcdic`] passes with the record count of its ASCII
/// file.
fn check_ebcdic<L: Layout>(ebcdic: &str) -> Vec<L::Record> {
    let counterpart = EBCDIC_COUNTERPARTS
        .iter()
        .find(|counterpart| counterpart.ebcdic == ebcdic)
        .unwrap_or_else(|| panic!("{ebcdic} is not an EBCDIC counterpart file"));
    assert_eq!(
        counterpart.record_len,
        L::LEN,
        "{ebcdic}: record length of {}",
        L::NAME
    );
    assert_eq!(
        read_ebcdic(ebcdic).len(),
        counterpart.file_bytes,
        "{ebcdic}: file size"
    );
    roundtrip_ebcdic::<L>(ebcdic, ascii_count(counterpart.ascii))
}

#[test]
fn ascii_corpus_totals_626() {
    let total: usize = ASCII_COUNTS.iter().map(|&(_, count)| count).sum();
    assert_eq!(total, CORPUS_RECORDS, "ASCII corpus record count");
    for &(file, count) in &ASCII_COUNTS {
        let bytes = read_ascii(file);
        assert_eq!(split_ascii(&bytes).len(), count, "{file}: record count");
    }
}

#[test]
fn every_ascii_record_has_a_line_end() {
    for &(file, _) in &ASCII_COUNTS {
        let bytes = read_ascii(file);
        for (&(frame, _), n) in split_ascii(&bytes).iter().zip(1_usize..) {
            assert_ne!(
                frame.terminator,
                Terminator::None,
                "{file} record {n}: no line end"
            );
        }
    }
}

#[test]
fn acctdata_roundtrips() {
    let records =
        check_ascii::<AccountLayout>("acctdata", 15_050, |_| AsciiFrame::lf(AccountLayout::LEN));
    assert_nothing_preserved("acctdata.txt", records.iter().map(|framed| &framed.record));
}

#[test]
fn carddata_roundtrips() {
    check_ascii::<CardLayout>("carddata", 7550, |_| AsciiFrame::lf(CardLayout::LEN));
}

#[test]
fn cardxref_roundtrips_from_its_36_byte_records() {
    let records =
        check_ascii::<CardXrefLayout>("cardxref", 1850, |_| AsciiFrame::lf(CARDXREF_STORED_LEN));
    for (framed, n) in records.iter().zip(1_usize..) {
        assert_eq!(
            framed.record.filler.as_bytes(),
            &XREF_FILLER,
            "cardxref.txt record {n}: the absent FILLER decodes as spaces"
        );
    }
}

#[test]
fn custdata_roundtrips() {
    check_ascii::<CustomerLayout>("custdata", 25_050, |_| AsciiFrame::lf(CustomerLayout::LEN));
}

#[test]
fn dailytran_roundtrips() {
    let records = check_ascii::<DailyTranLayout>("dailytran", 105_300, |_| {
        AsciiFrame::lf(DailyTranLayout::LEN)
    });
    assert_nothing_preserved("dailytran.txt", records.iter().map(|framed| &framed.record));
}

#[test]
fn discgrp_roundtrips() {
    let records = check_ascii::<DisclosureGroupLayout>("discgrp", 2601, |_| {
        AsciiFrame::lf(DisclosureGroupLayout::LEN)
    });
    assert_nothing_preserved("discgrp.txt", records.iter().map(|framed| &framed.record));
}

#[test]
fn tcatbal_roundtrips_with_crlf_until_its_last_record() {
    let records = check_ascii::<TranCatBalanceLayout>("tcatbal", 2599, |n| {
        crlf_until_last(n, 50, TranCatBalanceLayout::LEN)
    });
    assert_nothing_preserved("tcatbal.txt", records.iter().map(|framed| &framed.record));
}

#[test]
fn trancatg_roundtrips_with_crlf_throughout() {
    check_ascii::<TranCategoryLayout>("trancatg", 1116, |_| AsciiFrame {
        stored_len: TranCategoryLayout::LEN,
        terminator: Terminator::CrLf,
    });
}

#[test]
fn trantype_roundtrips_with_crlf_until_its_last_record() {
    check_ascii::<TranTypeLayout>("trantype", 433, |n| {
        crlf_until_last(n, 7, TranTypeLayout::LEN)
    });
}

#[test]
fn ebcdic_corpus_totals_626() {
    let mut total = 0;
    for counterpart in &EBCDIC_COUNTERPARTS {
        let file = counterpart.ebcdic;
        let len = read_ebcdic(file).len();
        assert_eq!(len, counterpart.file_bytes, "{file}: file size");
        assert!(
            len.is_multiple_of(counterpart.record_len),
            "{file}: {len} bytes is not a whole number of {}-byte records",
            counterpart.record_len
        );
        let count = len / counterpart.record_len;
        assert_eq!(
            count,
            ascii_count(counterpart.ascii),
            "{file}: record count against {}",
            counterpart.ascii
        );
        total += count;
    }
    assert_eq!(total, CORPUS_RECORDS, "EBCDIC corpus record count");
}

#[test]
fn ebcdic_acctdata_roundtrips() {
    let records = check_ebcdic::<AccountLayout>("AWS.M2.CARDDEMO.ACCTDATA.PS");
    assert_nothing_preserved("AWS.M2.CARDDEMO.ACCTDATA.PS", &records);
}

#[test]
fn ebcdic_carddata_roundtrips() {
    check_ebcdic::<CardLayout>("AWS.M2.CARDDEMO.CARDDATA.PS");
}

#[test]
fn ebcdic_cardxref_roundtrips() {
    let records = check_ebcdic::<CardXrefLayout>("AWS.M2.CARDDEMO.CARDXREF.PS");
    for (record, n) in records.iter().zip(1_usize..) {
        assert_eq!(
            record.filler.as_bytes(),
            &XREF_FILLER,
            "AWS.M2.CARDDEMO.CARDXREF.PS record {n}: the 0x40 FILLER decodes as spaces"
        );
    }
}

#[test]
fn ebcdic_custdata_roundtrips() {
    check_ebcdic::<CustomerLayout>("AWS.M2.CARDDEMO.CUSTDATA.PS");
}

#[test]
fn ebcdic_dalytran_roundtrips() {
    let records = check_ebcdic::<DailyTranLayout>("AWS.M2.CARDDEMO.DALYTRAN.PS");
    assert_nothing_preserved("AWS.M2.CARDDEMO.DALYTRAN.PS", &records);
}

#[test]
fn ebcdic_discgrp_roundtrips() {
    let records = check_ebcdic::<DisclosureGroupLayout>("AWS.M2.CARDDEMO.DISCGRP.PS");
    assert_nothing_preserved("AWS.M2.CARDDEMO.DISCGRP.PS", &records);
}

#[test]
fn ebcdic_tcatbalf_roundtrips() {
    let records = check_ebcdic::<TranCatBalanceLayout>("AWS.M2.CARDDEMO.TCATBALF.PS");
    assert_nothing_preserved("AWS.M2.CARDDEMO.TCATBALF.PS", &records);
}

#[test]
fn ebcdic_trancatg_roundtrips() {
    check_ebcdic::<TranCategoryLayout>("AWS.M2.CARDDEMO.TRANCATG.PS");
}

#[test]
fn ebcdic_trantype_roundtrips() {
    check_ebcdic::<TranTypeLayout>("AWS.M2.CARDDEMO.TRANTYPE.PS");
}

/// `CVTRA05Y` has the field widths of `CVTRA06Y` and no fixture of its own, so
/// the daily transactions exercise it in both character sets.
#[test]
fn transaction_layout_over_dailytran() {
    let ascii = roundtrip_ascii::<TransactionLayout>("dailytran.txt", ascii_count("dailytran.txt"));
    assert_frames("dailytran.txt", &ascii, |_| {
        AsciiFrame::lf(TransactionLayout::LEN)
    });
    assert_nothing_preserved("dailytran.txt", ascii.iter().map(|framed| &framed.record));
    let ebcdic = roundtrip_ebcdic::<TransactionLayout>(
        "AWS.M2.CARDDEMO.DALYTRAN.PS",
        ascii_count("dailytran.txt"),
    );
    assert_nothing_preserved("AWS.M2.CARDDEMO.DALYTRAN.PS", &ebcdic);
}

#[test]
fn accdata_equals_acctdata() {
    let copy = read_ebcdic("AWS.M2.CARDDEMO.ACCDATA.PS");
    let counterpart = read_ebcdic("AWS.M2.CARDDEMO.ACCTDATA.PS");
    assert_eq!(copy.len(), 15_000, "AWS.M2.CARDDEMO.ACCDATA.PS: file size");
    assert!(
        copy == counterpart,
        "ACCDATA.PS differs from ACCTDATA.PS at offset {}: it may hide another layout; an \
         escalation is needed in docs/parity/escalations.md",
        first_difference(&copy, &counterpart)
    );
}

/// The ten `USRSEC` rows round-trip through `CSUSR01Y`. The password field is
/// never read: only the user IDs and types are compared, and no record is
/// printed or snapshotted.
#[test]
fn usrsec_roundtrips_through_user_security_layout() {
    let file = "AWS.M2.CARDDEMO.USRSEC.PS";
    assert_eq!(read_ebcdic(file).len(), 800, "{file}: file size");
    let records = roundtrip_ebcdic::<UserSecurityLayout>(file, USERS.len());
    let users: Vec<(String, String)> = records
        .iter()
        .map(|record| {
            (
                record.sec_usr_id.to_string(),
                record.sec_usr_type.to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = USERS
        .iter()
        .map(|&(id, user_type)| (id.to_owned(), user_type.to_owned()))
        .collect();
    assert_eq!(users, expected, "{file}: user IDs and types in file order");
}

#[test]
fn ebcdic_directory_is_classified() {
    let classified: BTreeSet<&str> = EBCDIC_COUNTERPARTS
        .iter()
        .map(|counterpart| counterpart.ebcdic)
        .chain(EBCDIC_EXCLUDED)
        .collect();
    let mut found = BTreeSet::new();
    let entries =
        std::fs::read_dir(EBCDIC_DIR).unwrap_or_else(|e| panic!("cannot list {EBCDIC_DIR}: {e}"));
    for entry in entries {
        let name = entry
            .unwrap_or_else(|e| panic!("cannot list {EBCDIC_DIR}: {e}"))
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert!(
            name == GITKEEP || classified.contains(name.as_str()),
            "{EBCDIC_DIR}/{name} is neither one of the nine counterpart files nor one of the \
             four excluded files: classify it in this test"
        );
        found.insert(name);
    }
    for &name in &classified {
        assert!(found.contains(name), "{name} is missing from {EBCDIC_DIR}");
    }
}
