//! Criterion benchmark of the codec A primitives over the ASCII fixture corpus (AAP 0.3.3.5).
//!
//! The fixtures are read in place from `app/data/ASCII/`, each exactly once and never copied.
//! Four groups time the primitives the record codecs are built from:
//!
//! - `framing`: [`lines`] over every fixture file, and rebuilding the file from its lines;
//! - `zoned`: signed DISPLAY with a trailing overpunch, the `PIC S9(10)V99` money fields of
//!   `ACCOUNT-RECORD` (`CVACT01Y.cpy`) and `DALYTRAN-AMT` `PIC S9(09)V99` (`CVTRA06Y.cpy`);
//! - `dates`: [`PicDate`] over the `ACCOUNT-RECORD` dates and [`PicTimestamp`] over
//!   `DALYTRAN-ORIG-TS` and the all-blank `DALYTRAN-PROC-TS`;
//! - `packed`: `COMP-3` `PIC S9(09)V99` over deterministic generated values, since no ASCII
//!   fixture holds packed bytes.
//!
//! Before anything is timed, every group proves its inputs clean: line counts match the corpus,
//! the lines rebuild each file byte for byte, every field decodes, and every decoded value
//! re-encodes to the bytes it came from. A timing is therefore never taken over an error path.
//!
//! Fixture lines hold card numbers, SSNs and other cardholder data, so assertion and panic
//! messages name the file, field group and offsets only, never bytes or values, and byte buffers
//! are compared with `assert!` rather than `assert_eq!`, whose failure output would print them
//! (Cardholder Data Redaction). The card number fields are never sliced.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use copybook::framing::{Terminator, lines};
use copybook::zoned::{self, packed};
use copybook::{ErrorKind, PicDate, PicTimestamp};
use criterion::measurement::WallTime;
use criterion::{
    BenchmarkGroup, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main,
};
use rust_decimal::Decimal;

/// Every ASCII fixture file with the number of lines it holds.
const FIXTURES: [(&str, usize); 9] = [
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

/// The `ACCOUNT-RECORD` fixture (`CVACT01Y.cpy`).
const ACCTDATA: &str = "acctdata.txt";

/// The `DALYTRAN-RECORD` fixture (`CVTRA06Y.cpy`).
const DAILYTRAN: &str = "dailytran.txt";

/// Offsets of `ACCT-CURR-BAL`, `ACCT-CREDIT-LIMIT`, `ACCT-CASH-CREDIT-LIMIT`,
/// `ACCT-CURR-CYC-CREDIT` and `ACCT-CURR-CYC-DEBIT`, each `PIC S9(10)V99`.
const ACCT_MONEY_OFFSETS: [usize; 5] = [12, 24, 36, 78, 90];

/// Integer digits of the `ACCOUNT-RECORD` money fields.
const ACCT_MONEY_INT_DIGITS: u32 = 10;

/// Bytes of one `PIC S9(10)V99` DISPLAY field.
const ACCT_MONEY_WIDTH: usize = 12;

/// Offset of `DALYTRAN-AMT PIC S9(09)V99`.
const DALYTRAN_AMT_OFFSET: usize = 132;

/// Integer digits of `DALYTRAN-AMT`.
const DALYTRAN_AMT_INT_DIGITS: u32 = 9;

/// Bytes of one `PIC S9(09)V99` DISPLAY field.
const DALYTRAN_AMT_WIDTH: usize = 11;

/// Fraction digits of every money field benchmarked here.
const MONEY_SCALE: u32 = 2;

/// Offsets of `ACCT-OPEN-DATE`, `ACCT-EXPIRAION-DATE` and `ACCT-REISSUE-DATE`, each `PIC X(10)`.
const ACCT_DATE_OFFSETS: [usize; 3] = [48, 58, 68];

/// Offsets of `DALYTRAN-ORIG-TS` and `DALYTRAN-PROC-TS`, each `PIC X(26)`.
const DALYTRAN_TS_OFFSETS: [usize; 2] = [278, 304];

/// Total digits of the packed `PIC S9(09)V99` picture.
const PACKED_DIGITS: u32 = 11;

/// Bytes of one packed `PIC S9(09)V99` field.
const PACKED_WIDTH: usize = 6;

/// Generated packed values, after the edge values.
const PACKED_SAMPLES: usize = 1_024;

/// Edge values of `PIC S9(09)V99`, in cents: zero, the smallest magnitudes and the extremes.
const PACKED_EDGE_CENTS: [i64; 5] = [0, 1, -1, 99_999_999_999, -99_999_999_999];

/// Multiplier of the 64-bit linear congruential generator behind the packed values.
const LCG_MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// Increment of the same generator.
const LCG_INCREMENT: u64 = 1_442_695_040_888_963_407;

/// Bound on generated cents: 10^11, so every value fits `PIC S9(09)V99`.
const PACKED_CENTS_BOUND: u64 = 100_000_000_000;

/// The fixture bytes, in [`FIXTURES`] order, read on first use.
static CORPUS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();

/// `app/data/ASCII/`, resolved from this crate's manifest directory.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII")
}

/// Reads one fixture file. A failure names the file and the I/O error only.
fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_dir().join(name))
        .unwrap_or_else(|err| panic!("fixture {name}: cannot be read: {err}"))
}

/// Every fixture's bytes, in [`FIXTURES`] order.
fn corpus() -> &'static [Vec<u8>] {
    CORPUS.get_or_init(|| {
        FIXTURES
            .iter()
            .map(|&(name, _)| read_fixture(name))
            .collect()
    })
}

/// The bytes of fixture `name` and its expected line count.
fn fixture(name: &str) -> (&'static [u8], usize) {
    let (&(_, line_count), bytes) = FIXTURES
        .iter()
        .zip(corpus())
        .find(|((file, _), _)| *file == name)
        .unwrap_or_else(|| panic!("fixture {name}: not in the fixture table"));
    (bytes.as_slice(), line_count)
}

/// The `width` bytes at `offset` of a line body, or `None` when the body ends before them.
fn field(body: &[u8], offset: usize, width: usize) -> Option<&[u8]> {
    body.get(offset..offset.checked_add(width)?)
}

/// A byte count for [`Throughput::Bytes`].
fn byte_count(len: usize) -> u64 {
    u64::try_from(len).expect("a byte count fits 64 bits")
}

/// Total length of `slices`.
fn total_len(slices: &[&[u8]]) -> usize {
    slices.iter().map(|slice| slice.len()).sum()
}

/// Rebuilds a file from its split lines: each body followed by its terminator.
fn reencode(pairs: &[(&[u8], Terminator)], capacity: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(capacity);
    for &(body, terminator) in pairs {
        out.extend_from_slice(body);
        out.extend_from_slice(terminator.as_bytes());
    }
    out
}

/// The field at each of `offsets`, `width` bytes, on every line of fixture `file`, line by line.
///
/// # Panics
///
/// When the file's line count differs from [`FIXTURES`] or a body ends before a field.
fn field_slices(file: &str, offsets: &[usize], width: usize) -> Vec<&'static [u8]> {
    let (bytes, line_count) = fixture(file);
    let mut missing = 0_usize;
    let mut slices = Vec::with_capacity(line_count.saturating_mul(offsets.len()));
    let mut seen_lines = 0_usize;
    for line in lines(bytes) {
        seen_lines += 1;
        for &offset in offsets {
            match field(line.body, offset, width) {
                Some(slice) => slices.push(slice),
                None => missing += 1,
            }
        }
    }
    assert_eq!(seen_lines, line_count, "{file}: line count");
    assert_eq!(
        missing, 0,
        "{file} offsets {offsets:?}: fields missing from short bodies"
    );
    slices
}

/// Decodes every slice once and re-encodes every value, and returns the values.
///
/// # Panics
///
/// When any slice fails to decode, or any value re-encodes to bytes other than its slice. The
/// message carries `label` and the counts only.
fn preflight<T>(
    label: &str,
    slices: &[&[u8]],
    decode: impl Fn(&[u8]) -> Result<T, ErrorKind>,
    encode: impl Fn(&T, &mut Vec<u8>),
) -> Vec<T> {
    let mut errors = 0_usize;
    let mut mismatches = 0_usize;
    let mut values = Vec::with_capacity(slices.len());
    let mut out = Vec::new();
    for &slice in slices {
        match decode(slice) {
            Ok(value) => {
                out.clear();
                encode(&value, &mut out);
                if out.as_slice() != slice {
                    mismatches += 1;
                }
                values.push(value);
            }
            Err(_) => errors += 1,
        }
    }
    assert_eq!(errors, 0, "{label}: decode errors");
    assert_eq!(
        mismatches, 0,
        "{label}: re-encoded fields differ from their bytes"
    );
    values
}

/// Times `decode` over every slice; throughput is the slices' total length.
fn bench_decode<T>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    id: BenchmarkId,
    slices: &[&[u8]],
    decode: impl Fn(&[u8]) -> Result<T, ErrorKind>,
) {
    group.throughput(Throughput::Bytes(byte_count(total_len(slices))));
    group.bench_function(id, |b| {
        b.iter(|| {
            let mut decoded = 0_usize;
            for &slice in slices {
                if black_box(decode(black_box(slice))).is_ok() {
                    decoded += 1;
                }
            }
            decoded
        });
    });
}

/// Times `encode` of every value into one buffer, cleared each iteration; throughput is
/// `encoded_len`, the bytes one pass appends.
fn bench_encode<T>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    id: BenchmarkId,
    values: &[T],
    encoded_len: usize,
    encode: impl Fn(&T, &mut Vec<u8>),
) {
    group.throughput(Throughput::Bytes(byte_count(encoded_len)));
    let mut out = Vec::with_capacity(encoded_len);
    group.bench_function(id, |b| {
        b.iter(|| {
            out.clear();
            for value in values {
                encode(black_box(value), &mut out);
            }
            black_box(out.len())
        });
    });
}

/// `framing`: splitting every fixture file into lines, and rebuilding it from them.
fn bench_framing(c: &mut Criterion) {
    let mut group = c.benchmark_group("framing");
    for (&(name, line_count), bytes) in FIXTURES.iter().zip(corpus()) {
        let split: Vec<(&[u8], Terminator)> = lines(bytes)
            .map(|line| (line.body, line.framing.terminator))
            .collect();
        assert_eq!(split.len(), line_count, "{name}: line count");
        assert!(
            reencode(&split, bytes.len()).as_slice() == bytes.as_slice(),
            "{name}: lines do not rebuild the file byte for byte"
        );
        group.throughput(Throughput::Bytes(byte_count(bytes.len())));
        group.bench_with_input(
            BenchmarkId::new("lines", name),
            bytes.as_slice(),
            |b, input| {
                b.iter(|| {
                    lines(black_box(input))
                        .fold(0_usize, |total, line| total + line.framing.body_len)
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("reencode", name),
            split.as_slice(),
            |b, pairs| {
                b.iter(|| reencode(black_box(pairs), bytes.len()));
            },
        );
    }
    group.finish();
}

/// `zoned`: signed DISPLAY decode and encode of the `ACCOUNT-RECORD` money fields (250) and
/// `DALYTRAN-AMT` (300).
fn bench_zoned(c: &mut Criterion) {
    let mut group = c.benchmark_group("zoned");
    let sets: [(&str, &str, &[usize], usize, u32); 2] = [
        (
            "acctdata_s9_10v99",
            ACCTDATA,
            &ACCT_MONEY_OFFSETS,
            ACCT_MONEY_WIDTH,
            ACCT_MONEY_INT_DIGITS,
        ),
        (
            "dailytran_s9_09v99",
            DAILYTRAN,
            &[DALYTRAN_AMT_OFFSET],
            DALYTRAN_AMT_WIDTH,
            DALYTRAN_AMT_INT_DIGITS,
        ),
    ];
    for (id, file, offsets, width, int_digits) in sets {
        let slices = field_slices(file, offsets, width);
        let decode = |bytes: &[u8]| zoned::decode_signed(bytes, int_digits, MONEY_SCALE);
        let encode = |value: &Decimal, out: &mut Vec<u8>| {
            zoned::encode_signed(*value, int_digits, MONEY_SCALE, out);
        };
        let label = format!("zoned {file} offsets {offsets:?}");
        let values = preflight(&label, &slices, decode, encode);
        bench_decode(
            &mut group,
            BenchmarkId::new("decode_signed", id),
            &slices,
            decode,
        );
        let encoded_len = total_len(&slices);
        let encode_id = BenchmarkId::new("encode_signed", id);
        bench_encode(&mut group, encode_id, &values, encoded_len, encode);
    }
    group.finish();
}

/// `dates`: [`PicDate`] decode of the three `ACCOUNT-RECORD` dates (150), and [`PicTimestamp`]
/// decode of `DALYTRAN-ORIG-TS` (300, layout `I`) and of `DALYTRAN-PROC-TS` (300, all blank).
fn bench_dates(c: &mut Criterion) {
    let mut group = c.benchmark_group("dates");
    let date_slices = field_slices(ACCTDATA, &ACCT_DATE_OFFSETS, PicDate::WIDTH);
    let date_label = format!("dates {ACCTDATA} offsets {ACCT_DATE_OFFSETS:?}");
    let dates = preflight(&date_label, &date_slices, PicDate::decode, |date, out| {
        date.encode_into(out);
    });
    let blank_dates = dates.iter().filter(|date| date.value().is_none()).count();
    assert_eq!(blank_dates, 0, "{date_label}: blank dates");
    let date_id = BenchmarkId::new("pic_date_decode", "acctdata");
    bench_decode(&mut group, date_id, &date_slices, PicDate::decode);

    let kinds = [
        ("dailytran_orig_ts", false),
        ("dailytran_proc_ts_blank", true),
    ];
    for (offset, (id, all_blank)) in DALYTRAN_TS_OFFSETS.into_iter().zip(kinds) {
        let slices = field_slices(DAILYTRAN, &[offset], PicTimestamp::WIDTH);
        let label = format!("dates {DAILYTRAN} offset {offset}");
        let stamps = preflight(
            &label,
            &slices,
            PicTimestamp::decode,
            PicTimestamp::encode_into,
        );
        let blanks = stamps
            .iter()
            .filter(|stamp| stamp.value().is_none())
            .count();
        let expected_blanks = if all_blank { stamps.len() } else { 0 };
        assert_eq!(blanks, expected_blanks, "{label}: blank timestamps");
        let stamp_id = BenchmarkId::new("pic_timestamp_decode", id);
        bench_decode(&mut group, stamp_id, &slices, PicTimestamp::decode);
    }
    group.finish();
}

/// Deterministic `PIC S9(09)V99` values: the edge values, then [`PACKED_SAMPLES`] values from a
/// 64-bit linear congruential generator, every odd-indexed one negated.
fn packed_values() -> Vec<Decimal> {
    let mut values: Vec<Decimal> = PACKED_EDGE_CENTS
        .iter()
        .map(|&cents| Decimal::new(cents, MONEY_SCALE))
        .collect();
    let mut state = 0_u64;
    for index in 0..PACKED_SAMPLES {
        state = state
            .wrapping_mul(LCG_MULTIPLIER)
            .wrapping_add(LCG_INCREMENT);
        // TRUNCATION: a sign conversion only: the remainder modulo `PACKED_CENTS_BOUND` is below
        // 10^11, far below `i64::MAX`, so the conversion into `i64` is exact and the `expect` is
        // never reached.
        let cents = i64::try_from((state >> 33) % PACKED_CENTS_BOUND)
            .expect("cents below 10^11 fit 64 signed bits");
        let signed = if index % 2 == 1 { -cents } else { cents };
        values.push(Decimal::new(signed, MONEY_SCALE));
    }
    values
}

/// `packed`: `COMP-3` `PIC S9(09)V99` decode and encode over [`packed_values`].
fn bench_packed(c: &mut Criterion) {
    let label = "packed S9(09)V99";
    let width = packed::byte_width(PACKED_DIGITS).expect("PIC S9(09)V99 has a COMP-3 width");
    assert_eq!(width, PACKED_WIDTH, "{label}: byte width");
    let values = packed_values();
    let encoded_len = values.len().saturating_mul(width);
    let mut encoded = Vec::with_capacity(encoded_len);
    for &value in &values {
        packed::encode(value, PACKED_DIGITS, MONEY_SCALE, &mut encoded);
    }
    assert_eq!(encoded.len(), encoded_len, "{label}: encoded length");
    let slices: Vec<&[u8]> = encoded.chunks_exact(width).collect();
    let decode = |bytes: &[u8]| packed::decode(bytes, PACKED_DIGITS, MONEY_SCALE);
    let encode = |value: &Decimal, out: &mut Vec<u8>| {
        packed::encode(*value, PACKED_DIGITS, MONEY_SCALE, out);
    };
    let decoded = preflight(label, &slices, decode, encode);
    let mismatches = decoded
        .iter()
        .zip(&values)
        .filter(|(back, value)| back != value)
        .count();
    assert_eq!(
        mismatches, 0,
        "{label}: decoded values differ from the encoded ones"
    );

    let mut group = c.benchmark_group("packed");
    bench_decode(
        &mut group,
        BenchmarkId::new("decode", "s9_09v99"),
        &slices,
        decode,
    );
    let encode_id = BenchmarkId::new("encode", "s9_09v99");
    bench_encode(&mut group, encode_id, &values, encoded_len, encode);
    group.finish();
}

criterion_group!(
    benches,
    bench_framing,
    bench_zoned,
    bench_dates,
    bench_packed
);
criterion_main!(benches);
