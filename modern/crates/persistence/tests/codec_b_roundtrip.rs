//! Binding d9: codec B, the persistence codec of `persistence::rows`, loses nothing (AAP 0.3.3.4,
//! 0.3.3.5, outcome o4).
//!
//! - `fixtures_round_trip_through_postgres` decodes every line of the eight load fixtures and of
//!   `dailytran.txt` with codec A into its `Framed` envelope, writes the record through the
//!   repository traits, reads it back by its key and re-encodes it inside the unchanged envelope.
//!   The 626 lines come back byte for byte: LF and CRLF terminators (E06), the 36-byte
//!   `cardxref.txt` bodies (E04), non-space FILLER (E05), overpunched signs, leading zeros, and
//!   dates and timestamps typed in their columns (E33). The envelope's `LineFraming` never
//!   reaches the database; only `Framed::record` passes through codec B. The ten `USRSEC`
//!   records of `DUSRSECJ.jcl` come back in every field but `SEC-USR-PWD`, which is stored only
//!   as its argon2id hash (E25).
//! - `generated_records_round_trip_through_postgres` writes and reads back 64 generated records
//!   of each of the nine layouts, each inside a transaction it then rolls back. The field
//!   generators draw from codec A's domains and weight in every boundary codec A weights, and a
//!   few more: every `PIC 9(n)` value, and every amount and rate of either sign, with zero and
//!   the extremes weighted in; every date of the years 0000 to 9999, calendar boundaries and leap
//!   days weighted in; blank dates and timestamps; timestamps in each of the layouts `I`, `D`
//!   and `S`, midnight and the last microsecond of the day weighted in; and `PIC X(n)` text,
//!   all-space values and trailing spaces weighted in.
//!
//! Each test runs on a `postgres:16` container of its own from `test_support::postgres`, whose
//! schema the single schema executor applies; this file issues no SQL. A failure names the file
//! and line, or the layout, with lengths and counts at most, never record bytes, card numbers,
//! SSNs, CVVs, passwords or hashes (Cardholder Data Redaction). A failing generated case prints
//! its synthetic record through `Debug`, which redacts every protected field.

use std::fmt;
use std::fs;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use copybook::{
    CobolTimestamp, FixedRecord, Framed, PicDate, PicTimestamp, PicU, PicX, TsForm, lines,
};
use domain::batch::loads::{
    HashedUser, decode_fixture, decode_usrsec, hash_user, usrsec_instream_lines,
};
use domain::money::{Money, Rate};
use domain::online::cocrdlic::CardRef;
use domain::password;
use domain::records::{
    AccountRecord, CardRecord, CardXrefRecord, CustomerRecord, DailyTransactionRecord,
    DisclosureGroupRecord, TranCatBalanceRecord, TransactionCategoryRecord, TransactionRecord,
    TransactionTypeRecord, UserEntry, UserSecurityRecord,
};
use domain::sensitive::{CardNumber, Cvv, Ssn};
use persistence::{
    AccountRepository, CardRepository, CardXrefRepository, CustomerRepository,
    DisclosureGroupRepository, PersistenceError, PgStore, TranCatBalanceRepository,
    TransactionCategoryRepository, TransactionRepository, TransactionTypeRepository,
    UserRepository,
};
use proptest::strategy::{Just, Strategy};
use proptest::test_runner::{Config, TestCaseError, TestRunner};
use proptest::{prop_assert, prop_oneof};
use rust_decimal::Decimal;
use test_support::fixtures::{dusrsecj_path, fixture_path};
use test_support::postgres::start_postgres;
use tokio::runtime::{Builder, Runtime};

/// Lines of the nine ASCII fixtures: 50 + 50 + 50 + 50 + 51 + 50 + 18 + 7 + 300 (FIXFRM-010).
const CORPUS_LINES: usize = 626;

/// In-stream `USRSEC` records of `DUSRSECJ.jcl` (FIXLOAD-009).
const USERS: usize = 10;

/// Generated records per layout.
const CASES: u32 = 64;

/// Every repository trait this file calls. Both executors implement each one: `PgStore` serves
/// the fixtures, and a `PgTx` that is rolled back serves the generated records.
trait Repositories:
    AccountRepository
    + CardRepository
    + CardXrefRepository
    + CustomerRepository
    + DisclosureGroupRepository
    + TranCatBalanceRepository
    + TransactionCategoryRepository
    + TransactionRepository
    + TransactionTypeRepository
{
}

impl<E> Repositories for E where
    E: AccountRepository
        + CardRepository
        + CardXrefRepository
        + CustomerRepository
        + DisclosureGroupRepository
        + TranCatBalanceRepository
        + TransactionCategoryRepository
        + TransactionRepository
        + TransactionTypeRepository
{
}

/// A layout codec B stores, with the repository calls that write one record and read it back by
/// its key. `PgStore` implements every repository trait under the same method names, so each
/// call names its trait.
trait Stored: Sized + Clone + PartialEq + fmt::Debug {
    /// The copybook of the layout, which names it in failure messages.
    const LAYOUT: &'static str;

    /// Inserts `record`.
    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError>;

    /// Reads back the stored record whose key is `record`'s key.
    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError>;
}

/// A load fixture of a `Stored` layout, and the repository calls that replace and list its
/// table.
trait Fixture: Stored + FixedRecord {
    /// The fixture file inside `app/data/ASCII`.
    const FILE: &'static str;
    /// Its line count, the rows its load writes (FIXLOAD-001 to FIXLOAD-008).
    const LINES: usize;

    /// Replaces every row of the table with `records`, returning the rows written.
    async fn replace_all(store: &mut PgStore, records: &[Self]) -> Result<u64, PersistenceError>;

    /// Every row of the table, as records.
    async fn find_all(store: &mut PgStore) -> Result<Vec<Self>, PersistenceError>;
}

impl Stored for AccountRecord {
    const LAYOUT: &'static str = "CVACT01Y";

    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError> {
        AccountRepository::insert(executor, record).await
    }

    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError> {
        AccountRepository::find_by_id(executor, record.id).await
    }
}

impl Fixture for AccountRecord {
    const FILE: &'static str = "acctdata.txt";
    const LINES: usize = 50;

    async fn replace_all(store: &mut PgStore, records: &[Self]) -> Result<u64, PersistenceError> {
        AccountRepository::replace_all(store, records).await
    }

    async fn find_all(store: &mut PgStore) -> Result<Vec<Self>, PersistenceError> {
        AccountRepository::find_all(store).await
    }
}

impl Stored for CardRecord {
    const LAYOUT: &'static str = "CVACT02Y";

    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError> {
        CardRepository::insert(executor, record).await
    }

    /// The record part of the card's entry; the handle beside it is no copybook field.
    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError> {
        let entry = CardRepository::find_by_id(executor, &record.num).await?;
        Ok(entry.map(|(_, card)| card))
    }
}

impl Fixture for CardRecord {
    const FILE: &'static str = "carddata.txt";
    const LINES: usize = 50;

    async fn replace_all(store: &mut PgStore, records: &[Self]) -> Result<u64, PersistenceError> {
        CardRepository::replace_all(store, records).await
    }

    async fn find_all(store: &mut PgStore) -> Result<Vec<Self>, PersistenceError> {
        let entries = CardRepository::find_all(store).await?;
        Ok(entries.into_iter().map(|(_, card)| card).collect())
    }
}

impl Stored for CardXrefRecord {
    const LAYOUT: &'static str = "CVACT03Y";

    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError> {
        CardXrefRepository::insert(executor, record).await
    }

    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError> {
        CardXrefRepository::find_by_id(executor, &record.card_num).await
    }
}

impl Fixture for CardXrefRecord {
    const FILE: &'static str = "cardxref.txt";
    const LINES: usize = 50;

    async fn replace_all(store: &mut PgStore, records: &[Self]) -> Result<u64, PersistenceError> {
        CardXrefRepository::replace_all(store, records).await
    }

    async fn find_all(store: &mut PgStore) -> Result<Vec<Self>, PersistenceError> {
        CardXrefRepository::find_all(store).await
    }
}

impl Stored for CustomerRecord {
    const LAYOUT: &'static str = "CVCUS01Y";

    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError> {
        CustomerRepository::insert(executor, record).await
    }

    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError> {
        CustomerRepository::find_by_id(executor, record.id).await
    }
}

impl Fixture for CustomerRecord {
    const FILE: &'static str = "custdata.txt";
    const LINES: usize = 50;

    async fn replace_all(store: &mut PgStore, records: &[Self]) -> Result<u64, PersistenceError> {
        CustomerRepository::replace_all(store, records).await
    }

    async fn find_all(store: &mut PgStore) -> Result<Vec<Self>, PersistenceError> {
        CustomerRepository::find_all(store).await
    }
}

impl Stored for DisclosureGroupRecord {
    const LAYOUT: &'static str = "CVTRA02Y";

    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError> {
        DisclosureGroupRepository::insert(executor, record).await
    }

    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError> {
        DisclosureGroupRepository::find_by_id(
            executor,
            &record.acct_group_id,
            &record.tran_type_cd,
            record.tran_cat_cd,
        )
        .await
    }
}

impl Fixture for DisclosureGroupRecord {
    const FILE: &'static str = "discgrp.txt";
    const LINES: usize = 51;

    async fn replace_all(store: &mut PgStore, records: &[Self]) -> Result<u64, PersistenceError> {
        DisclosureGroupRepository::replace_all(store, records).await
    }

    async fn find_all(store: &mut PgStore) -> Result<Vec<Self>, PersistenceError> {
        DisclosureGroupRepository::find_all(store).await
    }
}

impl Stored for TranCatBalanceRecord {
    const LAYOUT: &'static str = "CVTRA01Y";

    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError> {
        TranCatBalanceRepository::insert(executor, record).await
    }

    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError> {
        TranCatBalanceRepository::find_by_id(executor, record.acct_id, &record.type_cd, record.cd)
            .await
    }
}

impl Fixture for TranCatBalanceRecord {
    const FILE: &'static str = "tcatbal.txt";
    const LINES: usize = 50;

    async fn replace_all(store: &mut PgStore, records: &[Self]) -> Result<u64, PersistenceError> {
        TranCatBalanceRepository::replace_all(store, records).await
    }

    async fn find_all(store: &mut PgStore) -> Result<Vec<Self>, PersistenceError> {
        TranCatBalanceRepository::find_all(store).await
    }
}

impl Stored for TransactionCategoryRecord {
    const LAYOUT: &'static str = "CVTRA04Y";

    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError> {
        TransactionCategoryRepository::insert(executor, record).await
    }

    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError> {
        TransactionCategoryRepository::find_by_id(executor, &record.type_cd, record.cat_cd).await
    }
}

impl Fixture for TransactionCategoryRecord {
    const FILE: &'static str = "trancatg.txt";
    const LINES: usize = 18;

    async fn replace_all(store: &mut PgStore, records: &[Self]) -> Result<u64, PersistenceError> {
        TransactionCategoryRepository::replace_all(store, records).await
    }

    async fn find_all(store: &mut PgStore) -> Result<Vec<Self>, PersistenceError> {
        TransactionCategoryRepository::find_all(store).await
    }
}

impl Stored for TransactionTypeRecord {
    const LAYOUT: &'static str = "CVTRA03Y";

    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError> {
        TransactionTypeRepository::insert(executor, record).await
    }

    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError> {
        TransactionTypeRepository::find_by_id(executor, &record.tran_type).await
    }
}

impl Fixture for TransactionTypeRecord {
    const FILE: &'static str = "trantype.txt";
    const LINES: usize = 7;

    async fn replace_all(store: &mut PgStore, records: &[Self]) -> Result<u64, PersistenceError> {
        TransactionTypeRepository::replace_all(store, records).await
    }

    async fn find_all(store: &mut PgStore) -> Result<Vec<Self>, PersistenceError> {
        TransactionTypeRepository::find_all(store).await
    }
}

/// `transactions` has no load: POSTTRAN writes it one record at a time, so it is no `Fixture`.
impl Stored for TransactionRecord {
    const LAYOUT: &'static str = "CVTRA05Y";

    async fn insert<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<(), PersistenceError> {
        TransactionRepository::insert(executor, record).await
    }

    async fn find_by_key<E: Repositories>(
        executor: &mut E,
        record: &Self,
    ) -> Result<Option<Self>, PersistenceError> {
        TransactionRepository::find_by_id(executor, &record.id).await
    }
}

// audit: FIXFRM-001
// audit: FIXFRM-002
// audit: FIXFRM-003
// audit: FIXFRM-004
// audit: FIXFRM-005
// audit: FIXFRM-006
// audit: FIXFRM-007
// audit: FIXFRM-008
// audit: FIXFRM-009
// audit: FIXFRM-010
// audit: FIXFRM-011
// audit: FIXFRM-012
// audit: FIXFRM-013
// audit: FIXFRM-014
// audit: FIXFRM-015
// audit: FIXFRM-016
// audit: FIXFRM-017
// audit: FIXFRM-018
// audit: FIXFRM-019
// audit: FIXFRM-020
// audit: FIXFRM-021
// audit: FIXLOAD-001
// audit: FIXLOAD-002
// audit: FIXLOAD-003
// audit: FIXLOAD-004
// audit: FIXLOAD-005
// audit: FIXLOAD-006
// audit: FIXLOAD-007
// audit: FIXLOAD-008
// audit: FIXLOAD-009
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
// audit: LOADS-014
// audit: LOADS-027
// audit: LOADS-030
// audit: LOADS-031
// audit: LOADS-032
// audit: LOADS-033
// audit: LOADS-034
// audit: PIC-001
// audit: PIC-003
// audit: PIC-005
// audit: PIC-006
// audit: PIC-012
// audit: PIC-013
// audit: PIC-014
// audit: PIC-015
// audit: PIC-017
// audit: PIC-018
// audit: PIC-023
// audit: PIC-024
// audit: CVACT02Y-008
// audit: CVACT03Y-004
// audit: CVTRA06Y-012
// audit: CVTRA06Y-013
// audit: CSUSR01Y-001
// audit: CSUSR01Y-002
// audit: CSUSR01Y-003
// audit: CSUSR01Y-004
// audit: CSUSR01Y-005
// audit: CSUSR01Y-006
#[tokio::test]
async fn fixtures_round_trip_through_postgres() {
    let guard = start_postgres()
        .await
        .expect("start a migrated postgres:16 container");
    let mut store = PgStore::new(guard.pool().clone());

    let lines = [
        accounts(&mut store).await,
        cards(&mut store).await,
        card_xrefs(&mut store).await,
        customers(&mut store).await,
        round_trip::<DisclosureGroupRecord>(&mut store).await.len(),
        tran_cat_balances(&mut store).await,
        round_trip::<TransactionCategoryRecord>(&mut store)
            .await
            .len(),
        round_trip::<TransactionTypeRecord>(&mut store).await.len(),
        daily_transactions(&mut store).await,
    ];
    assert_eq!(
        lines.iter().sum::<usize>(),
        CORPUS_LINES,
        "fixture lines that round-tripped"
    );

    users(&mut store).await;
}

/// The d9 steps every load fixture shares, over the fixture of `R`, returning its records in line
/// order.
///
/// `replace_all` writes the decoded records, and each one is read back by its key in line order:
/// `find_all` follows the table's key order, not the file's. The record read back must equal the
/// record written and, inside its line's unchanged envelope, re-encode to the line's bytes.
/// `find_all` must then return exactly the fixture's records, and so must it after a second
/// `replace_all` over the loaded table (LOADS-014).
async fn round_trip<R: Fixture>(store: &mut PgStore) -> Vec<R> {
    let file = R::FILE;
    let bytes = read(&fixture_path(file));
    let framed = decode::<R>(file, &bytes, R::LINES);
    let records: Vec<R> = framed.iter().map(|line| line.record.clone()).collect();
    let count = row_count(&records);

    let written = R::replace_all(store, &records)
        .await
        .unwrap_or_else(|err| panic!("{file}: replace_all: {err}"));
    assert_eq!(written, count, "{file}: rows replace_all wrote");

    let mut read_back = Vec::with_capacity(records.len());
    for (number, record) in (1_usize..).zip(&records) {
        let found = R::find_by_key(store, record)
            .await
            .unwrap_or_else(|err| panic!("{file}:{number} read by its key: {err}"));
        let Some(found) = found else {
            panic!("{file}:{number} is not found by its key");
        };
        assert!(
            found == *record,
            "{file}:{number} differs after the round trip"
        );
        read_back.push(found);
    }
    assert_reencodes(file, &bytes, &framed, read_back);
    assert_listed(store, &records).await;

    let rewritten = R::replace_all(store, &records)
        .await
        .unwrap_or_else(|err| panic!("{file}: second replace_all: {err}"));
    assert_eq!(rewritten, count, "{file}: rows a second replace_all wrote");
    assert_listed(store, &records).await;
    records
}

/// Checks that `find_all` returns each of `records` exactly once, in any order, and no other row.
async fn assert_listed<R: Fixture>(store: &mut PgStore, records: &[R]) {
    let file = R::FILE;
    let rows = R::find_all(store)
        .await
        .unwrap_or_else(|err| panic!("{file}: find_all: {err}"));
    assert_eq!(rows.len(), records.len(), "{file}: rows find_all returned");
    let mut unmatched: Vec<&R> = records.iter().collect();
    for (number, row) in (1_usize..).zip(&rows) {
        let Some(position) = unmatched.iter().position(|record| *record == row) else {
            panic!("{file}: row {number} of find_all matches no fixture record");
        };
        unmatched.swap_remove(position);
    }
}

/// Re-encodes each record read back inside the envelope codec A decoded its line into, and checks
/// the bytes against that line, then the whole file against the fixture (PIC-013, PIC-014).
fn assert_reencodes<R: FixedRecord>(
    file: &str,
    bytes: &[u8],
    framed: &[Framed<R>],
    read_back: Vec<R>,
) {
    let originals: Vec<_> = lines(bytes).collect();
    assert_eq!(
        originals.len(),
        framed.len(),
        "{file}: lines against envelopes"
    );
    assert_eq!(
        read_back.len(),
        framed.len(),
        "{file}: records against envelopes"
    );
    let mut encoded = Vec::with_capacity(bytes.len());
    let paired = originals.iter().zip(framed).zip(read_back);
    for (number, ((original, envelope), record)) in (1_usize..).zip(paired) {
        let line = Framed {
            record,
            framing: envelope.framing,
        }
        .try_encode()
        .unwrap_or_else(|err| panic!("{file}:{number} does not re-encode: {err}"));
        let expected = [original.body, original.framing.terminator.as_bytes()].concat();
        assert!(
            line == expected,
            "{file}:{number} re-encodes to other bytes than the fixture line"
        );
        encoded.extend_from_slice(&line);
    }
    assert_same_bytes(file, bytes, &encoded);
}

/// Checks that `encoded` is `original` byte for byte. The message gives the lengths and the first
/// differing offset, never a byte (Cardholder Data Redaction).
fn assert_same_bytes(file: &str, original: &[u8], encoded: &[u8]) {
    let first_difference = original
        .iter()
        .zip(encoded)
        .position(|(left, right)| left != right);
    assert!(
        original == encoded,
        "{file}: {} bytes re-encoded against {} in the fixture, first difference at offset \
         {first_difference:?}",
        encoded.len(),
        original.len()
    );
}

/// The whole file at `path`, read in place. The message names the file, never its contents.
fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Codec A over every line of `file`, which must hold `expected` lines. A `LoadError` names the
/// line and the field, never a byte.
fn decode<R: FixedRecord>(file: &str, bytes: &[u8], expected: usize) -> Vec<Framed<R>> {
    let framed = decode_fixture::<R>(bytes)
        .unwrap_or_else(|err| panic!("{file}: codec A refused the fixture: {err}"));
    assert_eq!(framed.len(), expected, "{file}: lines");
    framed
}

/// The count `replace_all` reports for `records`.
fn row_count<R>(records: &[R]) -> u64 {
    // TRUNCATION: none survives. The conversion is checked: `usize` is at most 64 bits on every
    // supported target, so every length converts into `u64` exactly, and a length `u64` cannot
    // hold would panic through `expect` instead of wrapping.
    u64::try_from(records.len()).expect("a record count fits u64")
}

/// The record of line 1 of `file`.
fn first_record<'r, R>(file: &str, records: &'r [R]) -> &'r R {
    records
        .first()
        .unwrap_or_else(|| panic!("{file} holds no record"))
}

/// `acctdata.txt` through `accounts`, then line 1's account updated to a negative balance and
/// back, and an update of an account no row holds (FIXLOAD-001).
async fn accounts(store: &mut PgStore) -> usize {
    let file = AccountRecord::FILE;
    let records = round_trip::<AccountRecord>(store).await;
    let first = first_record(file, &records);
    let changed = AccountRecord {
        curr_bal: Money::new(Decimal::new(-123_456, 2)).expect("-1234.56 fits S9(10)V99"),
        ..first.clone()
    };
    assert!(
        changed != *first,
        "{file}:1 already holds the changed balance"
    );
    for version in [&changed, first] {
        let updated = AccountRepository::update(store, version)
            .await
            .expect("accounts: update");
        assert!(updated, "{file}:1: the update matched no row");
        let found = AccountRepository::find_by_id(store, first.id)
            .await
            .expect("accounts: find_by_id");
        assert!(
            found.as_ref() == Some(version),
            "{file}:1 differs after an update"
        );
    }
    let absent = AccountRecord {
        id: PicU::new(99_999_999_999).expect("11 nines fit 9(11)"),
        ..first.clone()
    };
    let updated = AccountRepository::update(store, &absent)
        .await
        .expect("accounts: update");
    assert!(
        !updated,
        "an update of an account no row holds matched a row"
    );
    records.len()
}

/// `carddata.txt` through `cards`. A reload gives every card a new handle (LOADS-027), the
/// handle `find_by_id` returns reads the same card through `find_by_ref`, and line 1's card is
/// updated to another embossed name and back (FIXLOAD-002, CVACT02Y-008).
async fn cards(store: &mut PgStore) -> usize {
    let file = CardRecord::FILE;
    let records = round_trip::<CardRecord>(store).await;
    let first = first_record(file, &records);

    let handles = card_handles(store).await;
    let rewritten = CardRepository::replace_all(store, &records)
        .await
        .expect("cards: replace_all");
    assert_eq!(
        rewritten,
        row_count(&records),
        "{file}: rows a reload wrote"
    );
    let reloaded = card_handles(store).await;
    assert!(
        handles.iter().all(|handle| !reloaded.contains(handle)),
        "{file}: a reload kept a card handle"
    );

    let entry = CardRepository::find_by_id(store, &first.num)
        .await
        .expect("cards: find_by_id");
    let Some((handle, card)) = entry else {
        panic!("{file}:1 is not found by its key");
    };
    assert!(card == *first, "{file}:1 differs after a reload");
    let by_handle = CardRepository::find_by_ref(store, handle)
        .await
        .expect("cards: find_by_ref");
    assert!(
        by_handle == Some((handle, first.clone())),
        "{file}:1 differs when read by its handle"
    );

    let changed = CardRecord {
        embossed_name: PicX::from_text("CHANGED EMBOSSED NAME").expect("21 bytes fit X(50)"),
        ..first.clone()
    };
    assert!(changed != *first, "{file}:1 already holds the changed name");
    for version in [&changed, first] {
        let updated = CardRepository::update(store, version)
            .await
            .expect("cards: update");
        assert!(updated, "{file}:1: the update matched no row");
        let found = CardRepository::find_by_ref(store, handle)
            .await
            .expect("cards: find_by_ref");
        assert!(
            found.as_ref().map(|(_, card)| card) == Some(version),
            "{file}:1 differs after an update"
        );
    }
    records.len()
}

/// The handles of every stored card, checked to be distinct.
async fn card_handles(store: &mut PgStore) -> Vec<CardRef> {
    let entries = CardRepository::find_all(store)
        .await
        .expect("cards: find_all");
    let handles: Vec<CardRef> = entries.iter().map(|(handle, _)| *handle).collect();
    let mut distinct = handles.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        handles.len(),
        "cards: two rows share a handle"
    );
    handles
}

/// `cardxref.txt` through `card_xref`, then line 1's account read back through the alternate
/// index on `acct_id` (FIXLOAD-003, LOADS-030).
async fn card_xrefs(store: &mut PgStore) -> usize {
    let file = CardXrefRecord::FILE;
    let records = round_trip::<CardXrefRecord>(store).await;
    let first = first_record(file, &records);
    let by_account = CardXrefRepository::find_first_two_by_acct_id(store, first.acct_id)
        .await
        .expect("card_xref: find_first_two_by_acct_id");
    assert!(
        by_account == [first.clone()],
        "{file}:1 is not the one cross-reference of its account"
    );
    assert!(
        by_account.iter().all(|xref| xref.acct_id == first.acct_id),
        "{file}: a cross-reference of another account was returned for line 1's account"
    );
    let one = CardXrefRepository::find_by_acct_id(store, first.acct_id)
        .await
        .expect("card_xref: find_by_acct_id");
    assert!(
        one.is_some_and(|xref| by_account.contains(&xref)),
        "{file}: the first cross-reference of line 1's account is not among its cross-references"
    );
    records.len()
}

/// `custdata.txt` through `customers`, then line 1's customer updated to another first name and
/// back (FIXLOAD-004).
async fn customers(store: &mut PgStore) -> usize {
    let file = CustomerRecord::FILE;
    let records = round_trip::<CustomerRecord>(store).await;
    let first = first_record(file, &records);
    let changed = CustomerRecord {
        first_name: PicX::from_text("CHANGED").expect("7 bytes fit X(25)"),
        ..first.clone()
    };
    assert!(changed != *first, "{file}:1 already holds the changed name");
    for version in [&changed, first] {
        let updated = CustomerRepository::update(store, version)
            .await
            .expect("customers: update");
        assert!(updated, "{file}:1: the update matched no row");
        let found = CustomerRepository::find_by_id(store, first.id)
            .await
            .expect("customers: find_by_id");
        assert!(
            found.as_ref() == Some(version),
            "{file}:1 differs after an update"
        );
    }
    records.len()
}

/// `tcatbal.txt` through `tran_cat_balances`, then line 1's balance updated to a negative amount
/// and back (FIXLOAD-006).
async fn tran_cat_balances(store: &mut PgStore) -> usize {
    let file = TranCatBalanceRecord::FILE;
    let records = round_trip::<TranCatBalanceRecord>(store).await;
    let first = first_record(file, &records);
    let changed = TranCatBalanceRecord {
        bal: Money::new(Decimal::new(-123_456, 2)).expect("-1234.56 fits S9(09)V99"),
        ..first.clone()
    };
    assert!(
        changed != *first,
        "{file}:1 already holds the changed balance"
    );
    for version in [&changed, first] {
        let updated = TranCatBalanceRepository::update(store, version)
            .await
            .expect("tran_cat_balances: update");
        assert!(updated, "{file}:1: the update matched no row");
        let found =
            TranCatBalanceRepository::find_by_id(store, first.acct_id, &first.type_cd, first.cd)
                .await
                .expect("tran_cat_balances: find_by_id");
        assert!(
            found.as_ref() == Some(version),
            "{file}:1 differs after an update"
        );
    }
    records.len()
}

/// `dailytran.txt` through the `transactions` row shape, the layouts of `CVTRA06Y` and
/// `CVTRA05Y` being identical (AAP 0.3.3.4): every daily record is inserted as a transaction,
/// read back by its ID and turned back into a daily record, which must equal the original and
/// re-encode to its line. Inserting line 1 again is a duplicate, and `find_last` returns the
/// highest ID (FIXFRM-005, FIXFRM-017).
async fn daily_transactions(store: &mut PgStore) -> usize {
    const FILE: &str = "dailytran.txt";
    const LINES: usize = 300;
    let bytes = read(&fixture_path(FILE));
    let framed = decode::<DailyTransactionRecord>(FILE, &bytes, LINES);

    let mut read_back = Vec::with_capacity(framed.len());
    for (number, line) in (1_usize..).zip(&framed) {
        let written = TransactionRecord::from(line.record.clone());
        TransactionRepository::insert(store, &written)
            .await
            .unwrap_or_else(|err| panic!("{FILE}:{number} insert: {err}"));
        let found = TransactionRepository::find_by_id(store, &written.id)
            .await
            .unwrap_or_else(|err| panic!("{FILE}:{number} read by its key: {err}"));
        let Some(found) = found else {
            panic!("{FILE}:{number} is not found by its key");
        };
        assert!(found == written, "{FILE}:{number} differs as a transaction");
        let daily = DailyTransactionRecord::from(found);
        assert!(
            daily == line.record,
            "{FILE}:{number} differs after the round trip"
        );
        read_back.push(daily);
    }
    assert_reencodes(FILE, &bytes, &framed, read_back);

    let first = TransactionRecord::from(first_record(FILE, &framed).record.clone());
    let duplicate = TransactionRepository::insert(store, &first).await;
    assert!(
        duplicate
            .as_ref()
            .is_err_and(PersistenceError::is_duplicate),
        "{FILE}:1 inserted twice: {duplicate:?}"
    );

    let highest = framed
        .iter()
        .max_by(|left, right| left.record.id.cmp(&right.record.id))
        .map(|line| TransactionRecord::from(line.record.clone()));
    let last = TransactionRepository::find_last(store)
        .await
        .expect("transactions: find_last");
    assert!(
        last.is_some() && last == highest,
        "{FILE}: find_last is not the record with the highest ID"
    );
    framed.len()
}

/// The ten `USRSEC` records of `DUSRSECJ.jcl` through `users` (FIXUSR-001 to FIXUSR-010,
/// LOADS-033, LOADS-034, E25). Every field but `SEC-USR-PWD` comes back as written, and the
/// password only as an argon2id hash that verifies it; the record rebuilt from the row and the
/// deck's own password bytes re-encodes to its deck line. The last user is then updated, deleted
/// and deleted again.
async fn users(store: &mut PgStore) {
    const FILE: &str = "DUSRSECJ.jcl";
    let deck = read(&dusrsecj_path());
    let framed = decode_usrsec(&deck).unwrap_or_else(|err| panic!("{FILE}: {err}"));
    let deck_lines = usrsec_instream_lines(&deck).unwrap_or_else(|err| panic!("{FILE}: {err}"));
    assert_eq!(framed.len(), USERS, "{FILE}: in-stream records");
    assert_eq!(deck_lines.len(), USERS, "{FILE}: in-stream lines");

    let hashed: Vec<HashedUser> = framed
        .iter()
        .map(|line| hash_user(line.record.clone()).unwrap_or_else(|err| panic!("{FILE}: {err}")))
        .collect();
    let written = UserRepository::replace_all(store, &hashed)
        .await
        .expect("users: replace_all");
    assert_eq!(
        written,
        row_count(&hashed),
        "{FILE}: rows replace_all wrote"
    );

    let mut last = None;
    for (number, (envelope, line)) in (1_usize..).zip(framed.iter().zip(&deck_lines)) {
        let record = &envelope.record;
        let entry = UserRepository::find_by_id(store, &record.id)
            .await
            .expect("users: find_by_id");
        let Some(entry) = entry else {
            panic!("{FILE} record {number} is not found by its key");
        };
        assert_stored_user(FILE, number, record, &entry);

        let rebuilt = UserSecurityRecord {
            id: entry.id.clone(),
            fname: entry.fname.clone(),
            lname: entry.lname.clone(),
            pwd: record.pwd.clone(),
            usr_type: entry.usr_type.clone(),
            filler: entry.filler.clone(),
        };
        let bytes = Framed {
            record: rebuilt,
            framing: envelope.framing,
        }
        .try_encode()
        .unwrap_or_else(|err| panic!("{FILE} record {number} does not re-encode: {err}"));
        let expected = [line.body, line.framing.terminator.as_bytes()].concat();
        assert!(
            bytes == expected,
            "{FILE} record {number} re-encodes to other bytes than its deck line"
        );
        last = Some(entry);
    }
    let last = last.unwrap_or_else(|| panic!("{FILE} holds no record"));
    update_and_delete_user(store, &last).await;
}

/// Checks the stored row of the deck's record `number` against the record: the five stored
/// fields byte for byte, and an argon2id hash in place of the password that does not hold its
/// bytes and verifies them (CSUSR01Y-001 to CSUSR01Y-006). Neither the password nor the hash is
/// ever printed.
fn assert_stored_user(file: &str, number: usize, record: &UserSecurityRecord, entry: &UserEntry) {
    assert!(
        entry.id == record.id
            && entry.fname == record.fname
            && entry.lname == record.lname
            && entry.usr_type == record.usr_type
            && entry.filler == record.filler,
        "{file} record {number}: a stored field differs after the round trip"
    );
    let plaintext = record.pwd.expose();
    assert!(
        entry.password_hash.starts_with("$argon2id$"),
        "{file} record {number}: the stored password is no argon2id hash"
    );
    assert!(
        !entry
            .password_hash
            .as_bytes()
            .windows(plaintext.len())
            .any(|window| window == plaintext.as_slice()),
        "{file} record {number}: the stored hash holds the password"
    );
    let verified = password::verify(plaintext, &entry.password_hash)
        .unwrap_or_else(|err| panic!("{file} record {number}: {err}"));
    assert!(
        verified,
        "{file} record {number}: the stored hash does not verify the password"
    );
}

/// Updates `entry`'s user to another first name and back, then deletes it: the delete removes
/// the row, and a second delete matches none.
async fn update_and_delete_user(store: &mut PgStore, entry: &UserEntry) {
    let changed = UserEntry {
        fname: PicX::from_text("CHANGED").expect("7 bytes fit X(20)"),
        ..entry.clone()
    };
    assert!(changed != *entry, "the user already holds the changed name");
    for version in [&changed, entry] {
        let updated = UserRepository::update(store, version)
            .await
            .expect("users: update");
        assert!(updated, "the user update matched no row");
        let found = UserRepository::find_by_id(store, &entry.id)
            .await
            .expect("users: find_by_id");
        assert!(
            found.as_ref() == Some(version),
            "the user differs after an update"
        );
    }
    let deleted = UserRepository::delete(store, &entry.id)
        .await
        .expect("users: delete");
    assert!(deleted, "the user delete matched no row");
    let found = UserRepository::find_by_id(store, &entry.id)
        .await
        .expect("users: find_by_id");
    assert!(found.is_none(), "a deleted user is still found");
    let deleted_again = UserRepository::delete(store, &entry.id)
        .await
        .expect("users: delete");
    assert!(!deleted_again, "a second delete of the user matched a row");
}

// audit: PIC-001
// audit: PIC-003
// audit: PIC-005
// audit: PIC-012
// audit: PIC-015
// audit: PIC-017
// audit: PIC-018
// audit: PIC-019
// audit: PIC-020
// audit: PIC-023
// audit: PIC-024
// audit: CVTRA05Y-012
// audit: CVTRA05Y-013
#[test]
fn generated_records_round_trip_through_postgres() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread Tokio runtime");
    // The container's removal needs a current runtime when the guard drops, which happens on
    // unwinding too, so the runtime stays entered for the whole test.
    let _entered = runtime.enter();
    let guard = runtime
        .block_on(start_postgres())
        .expect("start a migrated postgres:16 container");
    let store = PgStore::new(guard.pool().clone());

    generated(&runtime, &store, &account_record());
    generated(&runtime, &store, &card_record());
    generated(&runtime, &store, &card_xref_record());
    generated(&runtime, &store, &customer_record());
    generated(&runtime, &store, &disclosure_group_record());
    generated(&runtime, &store, &tran_cat_balance_record());
    generated(&runtime, &store, &transaction_category_record());
    generated(&runtime, &store, &transaction_type_record());
    generated(&runtime, &store, &transaction_record());
}

/// Writes and reads back `CASES` generated records of `R`, each inside a transaction that is
/// then rolled back, so no two cases share a key. `strategy` builds each record from the field
/// generators below, which draw from codec A's domains and weight in its boundaries. A failure
/// names the layout; proptest adds the minimal failing record, whose `Debug` redacts every
/// protected field.
fn generated<R: Stored>(runtime: &Runtime, store: &PgStore, strategy: &impl Strategy<Value = R>) {
    let layout = R::LAYOUT;
    let mut runner = TestRunner::new(Config {
        cases: CASES,
        failure_persistence: None,
        ..Config::default()
    });
    let outcome = runner.run(strategy, |record| {
        let found = runtime
            .block_on(write_and_read(store, &record))
            .map_err(|err| TestCaseError::fail(format!("{layout}: {err}")))?;
        prop_assert!(
            found.as_ref() == Some(&record),
            "{layout}: a generated record differs after the round trip"
        );
        Ok(())
    });
    if let Err(err) = outcome {
        panic!("{layout}: {err}");
    }
}

/// Inserts `record` inside a new transaction, reads it back by its key and rolls back.
async fn write_and_read<R: Stored>(
    store: &PgStore,
    record: &R,
) -> Result<Option<R>, PersistenceError> {
    let mut tx = store.begin().await?;
    R::insert(&mut tx, record).await?;
    let found = R::find_by_key(&mut tx, record).await?;
    tx.rollback().await?;
    Ok(found)
}

/// `PIC X(N)` of printable ASCII, the bytes a `CHAR(N)` column holds (PIC-001, PIC-002).
///
/// One value in ten is all spaces. Every other value draws its bytes independently, each a space
/// one time in four and otherwise any of the 95 printable bytes, space included, so a byte is a
/// space with probability about 0.26. About a third of all values, keys included, therefore end
/// in a space: `CHAR(n)` treats trailing spaces specially, and a key parameter compared as `text`
/// instead of `bpchar` finds no row for such a key. The independent draws alone make a value all
/// spaces only with probability 0.26 to the power `N`, about 0.07 for two bytes and under 0.005
/// from four bytes up, so the all-space arm is what makes all-space values of wide fields occur.
///
/// The bytes are drawn as a `Vec`, whose value tree lives on the heap: an array of `N` union
/// value trees, nested in a record's tuples, overflows a test thread's stack in a debug build.
fn pic_x<const N: usize>() -> impl Strategy<Value = PicX<N>> {
    let bytes = proptest::collection::vec(prop_oneof![1 => Just(b' '), 3 => 0x20_u8..=0x7E], N);
    prop_oneof![
        1 => Just(vec![b' '; N]),
        9 => bytes,
    ]
    .prop_map(|bytes| {
        let bytes = <[u8; N]>::try_from(bytes)
            .unwrap_or_else(|_| panic!("the strategy draws exactly N bytes"));
        PicX::from_array(bytes)
    })
}

/// `PIC 9(N)`: every value from 0 to `N` nines, zero and `N` nines weighted in (PIC-003,
/// PIC-023).
fn pic_u<const N: u32>() -> impl Strategy<Value = PicU<N>> {
    let max = PicU::<N>::MAX;
    prop_oneof![
        1 => Just(0),
        1 => Just(max),
        8 => 0..=max,
    ]
    .prop_map(|value| PicU::new(value).expect("a value of at most N nines"))
}

/// A `V99` decimal: every whole number of cents of at most `int_digits + 2` digits, either sign,
/// zero and both extremes weighted in.
fn scaled(int_digits: u32) -> impl Strategy<Value = Decimal> {
    let max = 10_i64.pow(int_digits + 2) - 1;
    prop_oneof![
        1 => Just(-max),
        1 => Just(0),
        1 => Just(max),
        7 => -max..=max,
    ]
    .prop_map(|cents| Decimal::new(cents, 2))
}

/// `PIC S9(N)V99` money: every amount the picture holds, zero and both extremes weighted in
/// (PIC-005).
fn money<const N: u32>() -> impl Strategy<Value = Money<N>> {
    scaled(N)
        .prop_map(|value| Money::new(value).expect("cents of at most N + 2 digits fit S9(N)V99"))
}

/// `DIS-INT-RATE PIC S9(04)V99`: every rate of at most six digits, either sign, zero and both
/// extremes weighted in (PIC-005).
fn rate() -> impl Strategy<Value = Rate<4>> {
    scaled(4).prop_map(|value| Rate::new(value).expect("six digits fit S9(04)V99"))
}

/// A card number, 16 ASCII digits.
fn card_number() -> impl Strategy<Value = CardNumber> {
    proptest::array::uniform::<_, 16>(b'0'..=b'9')
        .prop_map(|digits| CardNumber::new(PicX::from_array(digits)))
}

/// A calendar date with a four-digit year, `0000-01-01` to `9999-12-31`: every day of every year,
/// one draw in five taken from `calendar_boundaries`.
fn calendar_date() -> impl Strategy<Value = NaiveDate> {
    prop_oneof![
        1 => proptest::sample::select(calendar_boundaries()),
        4 => (0_i32..=9999, 1_u32..=366).prop_filter_map("a day of that year", |(year, ordinal)| {
            NaiveDate::from_yo_opt(year, ordinal)
        }),
    ]
}

/// The calendar boundaries and leap days a `DATE` or `TIMESTAMP(6)` column must keep: the first
/// and last dates four digits carry, the leap day of year 0 (a proleptic leap year), the last
/// day of February 1900 (a century year that is no leap year), the leap days of 2000 and 2024,
/// `2000-01-01`, the epoch the binary wire encoding of `DATE` and `TIMESTAMP(6)` counts from, and
/// the day before it, where the encoded offset changes sign.
fn calendar_boundaries() -> Vec<NaiveDate> {
    [
        (0, 1, 1),
        (0, 2, 29),
        (1900, 2, 28),
        (1999, 12, 31),
        (2000, 1, 1),
        (2000, 2, 29),
        (2024, 2, 29),
        (9999, 12, 31),
    ]
    .into_iter()
    .map(|(year, month, day)| {
        NaiveDate::from_ymd_opt(year, month, day).expect("every calendar boundary is a date")
    })
    .collect()
}

/// A time of day with microsecond precision and no leap second, midnight and the last
/// microsecond of the day weighted in.
fn time_of_day() -> impl Strategy<Value = NaiveTime> {
    let last = NaiveTime::from_hms_micro_opt(23, 59, 59, 999_999).expect("a time of day");
    prop_oneof![
        1 => Just(NaiveTime::MIN),
        1 => Just(last),
        8 => (0_u32..=23, 0_u32..=59, 0_u32..=59, 0_u32..=999_999).prop_map(
            |(hour, minute, second, micro)| {
                NaiveTime::from_hms_micro_opt(hour, minute, second, micro).expect("a time of day")
            },
        ),
    ]
}

/// A `PIC X(10)` date: blank (`NULL`) one time in five, otherwise a calendar date (`DATE`)
/// (PIC-015).
fn date() -> impl Strategy<Value = PicDate> {
    prop_oneof![
        1 => Just(PicDate::BLANK),
        4 => calendar_date().prop_map(|day| PicDate::new(Some(day)).expect("a four-digit year")),
    ]
}

/// A `PIC X(26)` timestamp: blank one time in five (PIC-017), otherwise a value on a
/// `calendar_date` in one of the layouts `I`, `D` and `S`, chosen evenly: a microsecond instant
/// in layout `I` or `D` (PIC-018, PIC-019), or the date at midnight in the screen-date layout
/// `S`, which carries no other time (PIC-020).
fn timestamp() -> impl Strategy<Value = PicTimestamp> {
    let form = proptest::sample::select(vec![TsForm::Iso, TsForm::Db2, TsForm::ScreenDate]);
    prop_oneof![
        1 => Just(PicTimestamp::BLANK),
        4 => (calendar_date(), time_of_day(), form).prop_map(|(day, clock, form)| {
            let clock = match form {
                TsForm::ScreenDate => NaiveTime::MIN,
                TsForm::Iso | TsForm::Db2 => clock,
            };
            stamp(day.and_time(clock), form)
        }),
    ]
}

/// The non-blank timestamp `value` in layout `form`.
fn stamp(value: NaiveDateTime, form: TsForm) -> PicTimestamp {
    PicTimestamp::new(Some(
        CobolTimestamp::new(value, form).expect("a microsecond instant of a four-digit year"),
    ))
}

/// `ACCOUNT-RECORD` (`CVACT01Y`).
fn account_record() -> impl Strategy<Value = AccountRecord> {
    (
        (
            pic_u::<11>(),
            pic_x::<1>(),
            money::<10>(),
            money::<10>(),
            money::<10>(),
        ),
        (date(), date(), date(), money::<10>(), money::<10>()),
        (pic_x::<10>(), pic_x::<10>(), pic_x::<178>()),
    )
        .prop_map(
            |(
                (id, active_status, curr_bal, credit_limit, cash_credit_limit),
                (open_date, expiraion_date, reissue_date, curr_cyc_credit, curr_cyc_debit),
                (addr_zip, group_id, filler),
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

/// `CARD-RECORD` (`CVACT02Y`).
fn card_record() -> impl Strategy<Value = CardRecord> {
    (
        card_number(),
        pic_u::<11>(),
        pic_u::<3>(),
        pic_x::<50>(),
        date(),
        pic_x::<1>(),
        pic_x::<59>(),
    )
        .prop_map(
            |(num, acct_id, cvv, embossed_name, expiraion_date, active_status, filler)| {
                CardRecord {
                    num,
                    acct_id,
                    cvv_cd: Cvv::new(cvv),
                    embossed_name,
                    expiraion_date,
                    active_status,
                    filler,
                }
            },
        )
}

/// `CARD-XREF-RECORD` (`CVACT03Y`).
fn card_xref_record() -> impl Strategy<Value = CardXrefRecord> {
    (card_number(), pic_u::<9>(), pic_u::<11>(), pic_x::<14>()).prop_map(
        |(card_num, cust_id, acct_id, filler)| CardXrefRecord {
            card_num,
            cust_id,
            acct_id,
            filler,
        },
    )
}

/// `CUSTOMER-RECORD` (`CVCUS01Y`).
fn customer_record() -> impl Strategy<Value = CustomerRecord> {
    (
        (pic_u::<9>(), pic_x::<25>(), pic_x::<25>(), pic_x::<25>()),
        (pic_x::<50>(), pic_x::<50>(), pic_x::<50>()),
        (
            pic_x::<2>(),
            pic_x::<3>(),
            pic_x::<10>(),
            pic_x::<15>(),
            pic_x::<15>(),
        ),
        (
            pic_u::<9>(),
            pic_x::<20>(),
            date(),
            pic_x::<10>(),
            pic_x::<1>(),
        ),
        (pic_u::<3>(), pic_x::<168>()),
    )
        .prop_map(
            |(
                (id, first_name, middle_name, last_name),
                (addr_line_1, addr_line_2, addr_line_3),
                (addr_state_cd, addr_country_cd, addr_zip, phone_num_1, phone_num_2),
                (ssn, govt_issued_id, dob_yyyy_mm_dd, eft_account_id, pri_card_holder_ind),
                (fico_credit_score, filler),
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
                ssn: Ssn::new(ssn),
                govt_issued_id,
                dob_yyyy_mm_dd,
                eft_account_id,
                pri_card_holder_ind,
                fico_credit_score,
                filler,
            },
        )
}

/// `DIS-GROUP-RECORD` (`CVTRA02Y`).
fn disclosure_group_record() -> impl Strategy<Value = DisclosureGroupRecord> {
    (
        pic_x::<10>(),
        pic_x::<2>(),
        pic_u::<4>(),
        rate(),
        pic_x::<28>(),
    )
        .prop_map(
            |(acct_group_id, tran_type_cd, tran_cat_cd, int_rate, filler)| DisclosureGroupRecord {
                acct_group_id,
                tran_type_cd,
                tran_cat_cd,
                int_rate,
                filler,
            },
        )
}

/// `TRAN-CAT-BAL-RECORD` (`CVTRA01Y`).
fn tran_cat_balance_record() -> impl Strategy<Value = TranCatBalanceRecord> {
    (
        pic_u::<11>(),
        pic_x::<2>(),
        pic_u::<4>(),
        money::<9>(),
        pic_x::<22>(),
    )
        .prop_map(|(acct_id, type_cd, cd, bal, filler)| TranCatBalanceRecord {
            acct_id,
            type_cd,
            cd,
            bal,
            filler,
        })
}

/// `TRAN-CAT-RECORD` (`CVTRA04Y`).
fn transaction_category_record() -> impl Strategy<Value = TransactionCategoryRecord> {
    (pic_x::<2>(), pic_u::<4>(), pic_x::<50>(), pic_x::<4>()).prop_map(
        |(type_cd, cat_cd, type_desc, filler)| TransactionCategoryRecord {
            type_cd,
            cat_cd,
            type_desc,
            filler,
        },
    )
}

/// `TRAN-TYPE-RECORD` (`CVTRA03Y`).
fn transaction_type_record() -> impl Strategy<Value = TransactionTypeRecord> {
    (pic_x::<2>(), pic_x::<50>(), pic_x::<8>()).prop_map(|(tran_type, desc, filler)| {
        TransactionTypeRecord {
            tran_type,
            desc,
            filler,
        }
    })
}

/// `TRAN-RECORD` (`CVTRA05Y`), both timestamps in any of their layouts.
fn transaction_record() -> impl Strategy<Value = TransactionRecord> {
    (
        (
            pic_x::<16>(),
            pic_x::<2>(),
            pic_u::<4>(),
            pic_x::<10>(),
            pic_x::<100>(),
        ),
        (
            money::<9>(),
            pic_u::<9>(),
            pic_x::<50>(),
            pic_x::<50>(),
            pic_x::<10>(),
        ),
        (card_number(), timestamp(), timestamp(), pic_x::<20>()),
    )
        .prop_map(
            |(
                (id, type_cd, cat_cd, source, desc),
                (amt, merchant_id, merchant_name, merchant_city, merchant_zip),
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
