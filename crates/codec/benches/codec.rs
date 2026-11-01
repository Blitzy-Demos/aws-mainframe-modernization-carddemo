//! Criterion benchmarks of the `carddemo-codec` ASCII and EBCDIC record codecs
//! over the `CardDemo` fixture corpus (AAP 0.2.1, goal o4).
//!
//! Every benchmark reads the real fixture bytes (`app/data/ASCII/*.txt` and
//! `app/data/EBCDIC/AWS.M2.CARDDEMO.DALYTRAN.PS`) and proves its round trip
//! once before timing, so a broken codec fails the run instead of being
//! measured:
//!
//! - `dailytran`: `decode_ascii_file` and `encode_ascii_file` of the 300
//!   `CVTRA06Y` records of `dailytran.txt`.
//! - `dailytran_ebcdic`: `decode_ebcdic_file` and `encode_ebcdic_file` of the
//!   same 300 records in CP037, fixed 350-byte blocks.
//! - `corpus`: decode and re-encode of all nine ASCII fixture files, 626
//!   records in all.
//!
//! The record types stay opaque: the benchmarks compare bytes, never fields.

use std::hint::black_box;
use std::path::Path;
use std::sync::OnceLock;

use carddemo_codec::{
    AccountLayout, CardLayout, CardXrefLayout, CodecError, CustomerLayout, DailyTranLayout,
    DisclosureGroupLayout, Layout, TranCatBalanceLayout, TranCategoryLayout, TranTypeLayout,
    decode_ascii_file, decode_ebcdic_file, encode_ascii_file, encode_ebcdic_file,
};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};

/// The ASCII fixture corpus, read from the repository root.
const ASCII_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/data/ASCII");

/// The EBCDIC counterparts of the ASCII fixtures.
const EBCDIC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/data/EBCDIC");

/// The ASCII daily-transaction file, the largest of the corpus.
const DAILY_TRAN_ASCII: &str = "dailytran.txt";

/// The EBCDIC counterpart of `dailytran.txt`.
const DAILY_TRAN_EBCDIC: &str = "AWS.M2.CARDDEMO.DALYTRAN.PS";

/// The records of `dailytran.txt` and of its EBCDIC counterpart.
const DAILY_TRAN_RECORDS: usize = 300;

/// The records of the nine ASCII fixture files together.
const CORPUS_RECORDS: usize = 626;

/// Decodes an ASCII file with one layout and re-encodes it, returning the
/// record count and the re-encoded bytes.
type RoundTrip = fn(&[u8]) -> Result<(usize, Vec<u8>), CodecError>;

/// Each ASCII fixture file with the round trip of its layout.
const CORPUS: [(&str, RoundTrip); 9] = [
    ("acctdata.txt", roundtrip::<AccountLayout>),
    ("carddata.txt", roundtrip::<CardLayout>),
    ("cardxref.txt", roundtrip::<CardXrefLayout>),
    ("custdata.txt", roundtrip::<CustomerLayout>),
    (DAILY_TRAN_ASCII, roundtrip::<DailyTranLayout>),
    ("discgrp.txt", roundtrip::<DisclosureGroupLayout>),
    ("tcatbal.txt", roundtrip::<TranCatBalanceLayout>),
    ("trancatg.txt", roundtrip::<TranCategoryLayout>),
    ("trantype.txt", roundtrip::<TranTypeLayout>),
];

/// The fixture bytes, read once per process.
static FIXTURES: OnceLock<Fixtures> = OnceLock::new();

/// The fixture files every benchmark reads.
struct Fixtures {
    /// The nine ASCII files by name, in [`CORPUS`] order.
    ascii: [(&'static str, Vec<u8>); 9],
    /// `AWS.M2.CARDDEMO.DALYTRAN.PS`.
    daily_ebcdic: Vec<u8>,
}

impl Fixtures {
    fn load() -> Self {
        Self {
            ascii: CORPUS.map(|(name, _)| (name, read(ASCII_DIR, name))),
            daily_ebcdic: read(EBCDIC_DIR, DAILY_TRAN_EBCDIC),
        }
    }

    /// The bytes of the ASCII fixture file `name`.
    fn ascii(&self, name: &str) -> &[u8] {
        let Some((_, bytes)) = self.ascii.iter().find(|(stored, _)| *stored == name) else {
            panic!("{name} is not an ASCII fixture file of the corpus");
        };
        bytes
    }
}

fn fixtures() -> &'static Fixtures {
    FIXTURES.get_or_init(Fixtures::load)
}

fn read(dir: &str, name: &str) -> Vec<u8> {
    let path = Path::new(dir).join(name);
    std::fs::read(&path)
        .unwrap_or_else(|err| panic!("cannot read fixture {}: {err}", path.display()))
}

fn byte_len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("fixture length fits in u64")
}

fn roundtrip<L: Layout>(bytes: &[u8]) -> Result<(usize, Vec<u8>), CodecError> {
    let framed = decode_ascii_file::<L>(bytes)?;
    let encoded = encode_ascii_file::<L>(&framed)?;
    Ok((framed.len(), encoded))
}

fn bench_daily_tran_ascii(c: &mut Criterion) {
    let bytes = fixtures().ascii(DAILY_TRAN_ASCII);
    let framed = decode_ascii_file::<DailyTranLayout>(bytes)
        .unwrap_or_else(|err| panic!("{DAILY_TRAN_ASCII} does not decode: {err}"));
    assert_eq!(
        framed.len(),
        DAILY_TRAN_RECORDS,
        "{DAILY_TRAN_ASCII} record count"
    );
    let encoded = encode_ascii_file::<DailyTranLayout>(&framed)
        .unwrap_or_else(|err| panic!("{DAILY_TRAN_ASCII} does not re-encode: {err}"));
    assert!(
        encoded.as_slice() == bytes,
        "{DAILY_TRAN_ASCII} ASCII round trip is not byte-identical"
    );

    let mut group = c.benchmark_group("dailytran");
    group.throughput(Throughput::Bytes(byte_len(bytes)));
    group.bench_function("decode_ascii_file", |b| {
        b.iter(|| decode_ascii_file::<DailyTranLayout>(black_box(bytes)));
    });
    group.bench_function("encode_ascii_file", |b| {
        b.iter(|| encode_ascii_file::<DailyTranLayout>(black_box(&framed)));
    });
    group.finish();
}

fn bench_daily_tran_ebcdic(c: &mut Criterion) {
    let bytes = fixtures().daily_ebcdic.as_slice();
    let records = decode_ebcdic_file::<DailyTranLayout>(bytes)
        .unwrap_or_else(|err| panic!("{DAILY_TRAN_EBCDIC} does not decode: {err}"));
    assert_eq!(
        records.len(),
        DAILY_TRAN_RECORDS,
        "{DAILY_TRAN_EBCDIC} record count"
    );
    assert!(
        encode_ebcdic_file::<DailyTranLayout>(&records).as_slice() == bytes,
        "{DAILY_TRAN_EBCDIC} EBCDIC round trip is not byte-identical"
    );

    let mut group = c.benchmark_group("dailytran_ebcdic");
    group.throughput(Throughput::Bytes(byte_len(bytes)));
    group.bench_function("decode_ebcdic_file", |b| {
        b.iter(|| decode_ebcdic_file::<DailyTranLayout>(black_box(bytes)));
    });
    group.bench_function("encode_ebcdic_file", |b| {
        b.iter(|| encode_ebcdic_file::<DailyTranLayout>(black_box(&records)));
    });
    group.finish();
}

fn bench_corpus(c: &mut Criterion) {
    let fx = fixtures();
    // File lookups are resolved here so the timed loop runs only the codecs.
    let inputs = CORPUS.map(|(name, rt)| (name, rt, fx.ascii(name)));
    let mut total_records = 0;
    for &(name, rt, bytes) in &inputs {
        let (records, encoded) =
            rt(bytes).unwrap_or_else(|err| panic!("{name} does not round trip: {err}"));
        assert!(
            encoded.as_slice() == bytes,
            "{name} ASCII round trip is not byte-identical"
        );
        total_records += records;
    }
    assert_eq!(total_records, CORPUS_RECORDS, "corpus record count");
    let total_bytes = inputs
        .iter()
        .map(|&(_, _, bytes)| byte_len(bytes))
        .sum::<u64>();

    let mut group = c.benchmark_group("corpus");
    group.throughput(Throughput::Bytes(total_bytes));
    // The closure returns all nine results, so criterion keeps every one of
    // them observed and none of the round trips can be optimised away.
    group.bench_function("ascii_roundtrip", |b| {
        b.iter(|| inputs.map(|(_, rt, bytes)| rt(black_box(bytes))));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_daily_tran_ascii,
    bench_daily_tran_ebcdic,
    bench_corpus
);
criterion_main!(benches);
