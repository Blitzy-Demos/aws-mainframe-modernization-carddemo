//! [`AuthFraudRepository`]: the Db2 fraud table `CARDDEMO.AUTHFRDS`, table `auth_fraud` of
//! migration 0004 (`AUTHFRDS.ddl:1-28`), keyed by `(card_num, auth_ts)` (`AUTHFRDS.ddl:2-3,28`)
//! and served by the unique index `xauthfrd (card_num ASC, auth_ts DESC)` (`XAUTHFRD.ddl:1-4`).
//!
//! Only the COPAUS2C port writes the table: endpoint 23's service (CPVD F5), which COPAUS1C's
//! `MARK-AUTH-FRAUD` reaches through `EXEC CICS LINK` in the COBOL. Every statement mirrors one
//! of COPAUS2C's two (`app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl`):
//!
//! - [`insert`](AuthFraudRepository::insert) is `MAIN-PARA`'s `INSERT` of all 26 columns
//!   (`COPAUS2C.cbl:141-198`; COPAUS2C-007). A row already holding the key is SQLSTATE `23505`,
//!   [`PersistenceError::Duplicate`], the Db2 `SQLCODE -803` on which COPAUS2C performs
//!   `FRAUD-UPDATE` (`COPAUS2C.cbl:199-204`; COPAUS2C-008). This repository never retries: the
//!   insert-then-update decision is the service's, through `domain::online::copaus2c`'s
//!   `InsertResult::needs_update` and `fraud_outcome`. Any other error is COPAUS2C's
//!   insert-failure path (COPAUS2C-010).
//! - [`update_flag`](AuthFraudRepository::update_flag) is `FRAUD-UPDATE`, which sets
//!   `AUTH_FRAUD` and `FRAUD_RPT_DATE` of the row with the same key and nothing else
//!   (`COPAUS2C.cbl:221-229`; COPAUS2C-008). `Ok(false)` when no row has the key, the `SQLCODE`
//!   `+100` that COPAUS2C treats as a failure like any other non-zero code (`COPAUS2C.cbl:230-243`;
//!   COPAUS2C-011).
//! - [`find_by_id`](AuthFraudRepository::find_by_id) reads one row by its key. No COBOL program
//!   reads `AUTHFRDS`; this read observes what the two writes left, the stored report date
//!   included.
//!
//! # Values
//!
//! Codec B is `crate::rows::auth_fraud`: [`AuthFraudBinds`] for the insert and [`AuthFraudRow`]
//! for the read, every value moved verbatim (COPAUS2C-006).
//!
//! - `FRAUD_RPT_DATE` is Db2's `CURRENT DATE` in both COPAUS2C statements
//!   (`COPAUS2C.cbl:194,225`). Every date a rule writes comes from the injected `Clock` (AAP
//!   0.5.6), so both statements bind the `report_date` the service takes from its clock, and no
//!   statement reads the database's clock. Tests under `FixedClock` therefore assert the column.
//! - `auth_ts` is the typed `NaiveDateTime` the domain rebuilds from the detail's original date
//!   and inverted time (COPAUS2C-004), so no `TIMESTAMP_FORMAT` counterpart exists here (Typed
//!   Dates and Timestamps). `TIMESTAMP(6)` keeps microseconds and binding drops any finer part,
//!   so a key finer than a microsecond, or inside a leap second, names no storable instant: all
//!   three methods refuse it with a [`PersistenceError::Decode`] for `auth_ts` before any
//!   statement runs, rather than moving it to another instant (R7).
//! - The texts COPAUS2C builds from `SQLCODE` and `SQLSTATE` after a failure, `HALTED(H3)`, are
//!   the domain's `FraudOutcome::message`; this module reports only the typed error.
//!
//! # Transactions
//!
//! On a [`PgStore`] each statement commits on its own. On a [`PgTx`] a failed statement aborts the
//! whole transaction, a duplicate insert included, so endpoint 23's service runs
//! [`insert`](AuthFraudRepository::insert) inside a [`PgTx::savepoint`], rolls back to the
//! savepoint after [`PersistenceError::Duplicate`] and then calls
//! [`update_flag`](AuthFraudRepository::update_flag) in the same transaction. That keeps the
//! `auth_fraud` write and the `pending_auth_details` update in one unit of work, as COPAUS2C's
//! statements belong to COPAUS1C's (COPAUS2C-012).
//!
//! # Cardholder data
//!
//! `card_num` holds the full card number, which only the database keeps (COPAUS2C-013). It
//! reaches SQL only as a bind parameter, through [`CardNumber::expose`], and returns only inside
//! the [`CardNumber`] of the record. No error of this module carries a key, a timestamp or any
//! stored value, and the module emits no tracing (Cardholder Data Redaction).

use chrono::{NaiveDate, NaiveDateTime};
use copybook::PicX;
use domain::online::copaus2c::AuthFraudRow as FraudRecord;
use domain::sensitive::CardNumber;
use sqlx::PgExecutor;

use crate::error::PersistenceError;
use crate::rows::auth_fraud::storable_timestamp;
use crate::rows::{AuthFraudBinds, AuthFraudRow, pic_x_to_string};
use crate::{PgStore, PgTx, ProbePoint, probed};

/// The fraud rows of `auth_fraud`, as COPAUS2C writes them, implemented by [`PgStore`] and
/// [`PgTx`].
pub trait AuthFraudRepository: Send {
    /// `INSERT INTO CARDDEMO.AUTHFRDS`: one row holding every column of `record` and
    /// `report_date` as `fraud_rpt_date`, the stand-in for Db2's `CURRENT DATE`
    /// (`COPAUS2C.cbl:141-198`; COPAUS2C-007).
    ///
    /// On a [`PgTx`], a failure aborts the transaction; run the insert inside a
    /// [`PgTx::savepoint`] when the transaction must go on, as COPAUS2C's `-803` update does
    /// (COPAUS2C-008).
    ///
    /// # Errors
    ///
    /// - [`PersistenceError::Duplicate`] when a row already holds the `(card_num, auth_ts)` key,
    ///   Db2's `SQLCODE -803` (COPAUS2C-008).
    /// - [`PersistenceError::Decode`] when codec B refuses a value of `record`, such as a NUL or
    ///   non-ASCII byte or an `auth_ts` finer than a microsecond; nothing is written.
    /// - [`PersistenceError::Check`], [`PersistenceError::Database`] or
    ///   [`PersistenceError::Unavailable`] for any other failure (COPAUS2C-010).
    fn insert(
        &mut self,
        record: &FraudRecord,
        report_date: NaiveDate,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// `FRAUD-UPDATE`: sets `auth_fraud` to `flag` and `fraud_rpt_date` to `report_date` in the
    /// row keyed by `card` and `auth_ts`, leaving every other column as the row's first insert
    /// wrote it (`COPAUS2C.cbl:221-229`; COPAUS2C-008).
    ///
    /// Returns `true` when the row was updated and `false` when no row has the key, Db2's
    /// `SQLCODE +100` (COPAUS2C-011).
    ///
    /// # Errors
    ///
    /// - [`PersistenceError::Decode`] for an `auth_ts` that `TIMESTAMP(6)` cannot hold exactly,
    ///   or a NUL or non-ASCII byte in the card number or the flag; no statement runs.
    /// - [`PersistenceError::Database`] or [`PersistenceError::Unavailable`] for a failed
    ///   statement, such as one issued on a [`PgTx`] that an earlier failure aborted
    ///   (COPAUS2C-011).
    fn update_flag(
        &mut self,
        card: &CardNumber,
        auth_ts: NaiveDateTime,
        flag: &PicX<1>,
        report_date: NaiveDate,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// The row keyed by `card` and `auth_ts` as its record and its `fraud_rpt_date` (`None` for a
    /// `NULL` date), or `None` when no row has the key.
    ///
    /// # Errors
    ///
    /// - [`PersistenceError::Decode`] for an `auth_ts` that `TIMESTAMP(6)` cannot hold exactly or
    ///   a NUL or non-ASCII byte in the card number, before the statement runs; and for a stored
    ///   value its field cannot hold, a `NULL` outside `fraud_rpt_date` included.
    /// - [`PersistenceError::Database`] or [`PersistenceError::Unavailable`] for a failed
    ///   statement.
    fn find_by_id(
        &mut self,
        card: &CardNumber,
        auth_ts: NaiveDateTime,
    ) -> impl Future<Output = Result<Option<(FraudRecord, Option<NaiveDate>)>, PersistenceError>> + Send;
}

impl AuthFraudRepository for PgStore {
    fn insert(
        &mut self,
        record: &FraudRecord,
        report_date: NaiveDate,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AuthFraudInsert, &[record, &report_date]);
        probed(verdict, insert_one(self.pool(), record, report_date))
    }

    fn update_flag(
        &mut self,
        card: &CardNumber,
        auth_ts: NaiveDateTime,
        flag: &PicX<1>,
        report_date: NaiveDate,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        let verdict = self.verdict(
            ProbePoint::AuthFraudUpdate,
            &[card, &auth_ts, flag, &report_date],
        );
        probed(
            verdict,
            update_one(self.pool(), card, auth_ts, flag, report_date),
        )
    }

    fn find_by_id(
        &mut self,
        card: &CardNumber,
        auth_ts: NaiveDateTime,
    ) -> impl Future<Output = Result<Option<(FraudRecord, Option<NaiveDate>)>, PersistenceError>> + Send
    {
        select_by_id(self.pool(), card, auth_ts)
    }
}

impl AuthFraudRepository for PgTx<'_> {
    fn insert(
        &mut self,
        record: &FraudRecord,
        report_date: NaiveDate,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AuthFraudInsert, &[record, &report_date]);
        probed(verdict, insert_one(self.conn(), record, report_date))
    }

    fn update_flag(
        &mut self,
        card: &CardNumber,
        auth_ts: NaiveDateTime,
        flag: &PicX<1>,
        report_date: NaiveDate,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        let verdict = self.verdict(
            ProbePoint::AuthFraudUpdate,
            &[card, &auth_ts, flag, &report_date],
        );
        probed(
            verdict,
            update_one(self.conn(), card, auth_ts, flag, report_date),
        )
    }

    fn find_by_id(
        &mut self,
        card: &CardNumber,
        auth_ts: NaiveDateTime,
    ) -> impl Future<Output = Result<Option<(FraudRecord, Option<NaiveDate>)>, PersistenceError>> + Send
    {
        select_by_id(self.conn(), card, auth_ts)
    }
}

/// COPAUS2C's `INSERT`, the 26 columns in migration order, `fraud_rpt_date` from `report_date`
/// (`COPAUS2C.cbl:141-198`; COPAUS2C-007).
async fn insert_one<'e, E: PgExecutor<'e>>(
    executor: E,
    record: &FraudRecord,
    report_date: NaiveDate,
) -> Result<(), PersistenceError> {
    let binds = AuthFraudBinds::from_record(record)?;
    sqlx::query!(
        "INSERT INTO auth_fraud (
             card_num, auth_ts, auth_type, card_expiry_date, message_type, message_source,
             auth_id_code, auth_resp_code, auth_resp_reason, processing_code, transaction_amt,
             approved_amt, merchant_catagory_code, acqr_country_code, pos_entry_mode,
             merchant_id, merchant_name, merchant_city, merchant_state, merchant_zip,
             transaction_id, match_status, auth_fraud, fraud_rpt_date, acct_id, cust_id
         ) VALUES (
             $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,
             $19, $20, $21, $22, $23, $24::date, $25, $26
         )",
        binds.card_num,
        binds.auth_ts,
        binds.auth_type,
        binds.card_expiry_date,
        binds.message_type,
        binds.message_source,
        binds.auth_id_code,
        binds.auth_resp_code,
        binds.auth_resp_reason,
        binds.processing_code,
        binds.transaction_amt,
        binds.approved_amt,
        binds.merchant_catagory_code,
        binds.acqr_country_code,
        binds.pos_entry_mode,
        binds.merchant_id,
        binds.merchant_name,
        binds.merchant_city,
        binds.merchant_state,
        binds.merchant_zip,
        binds.transaction_id,
        binds.match_status,
        binds.auth_fraud,
        report_date,
        binds.acct_id,
        binds.cust_id,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// `FRAUD-UPDATE`: the flag and the report date of the row with the key, `true` when exactly one
/// row changed (`COPAUS2C.cbl:221-243`; COPAUS2C-008, COPAUS2C-011).
async fn update_one<'e, E: PgExecutor<'e>>(
    executor: E,
    card: &CardNumber,
    auth_ts: NaiveDateTime,
    flag: &PicX<1>,
    report_date: NaiveDate,
) -> Result<bool, PersistenceError> {
    let card_num = pic_x_to_string("card_num", card.expose())?;
    let auth_ts = storable_timestamp(auth_ts)?;
    let auth_fraud = pic_x_to_string("auth_fraud", flag)?;
    // The key text is cast to `bpchar`: `sqlx` binds a `String` as `text`, against which the
    // `CHAR(n)` column compares as `text`, its trailing spaces dropped, so a key that ends in
    // spaces would find no row and the primary key could not serve the statement.
    let done = sqlx::query!(
        "UPDATE auth_fraud SET auth_fraud = $1, fraud_rpt_date = $2
         WHERE card_num = $3::bpchar AND auth_ts = $4",
        auth_fraud,
        report_date,
        card_num,
        auth_ts,
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Every column of the row with the key, as the record and its report date.
async fn select_by_id<'e, E: PgExecutor<'e>>(
    executor: E,
    card: &CardNumber,
    auth_ts: NaiveDateTime,
) -> Result<Option<(FraudRecord, Option<NaiveDate>)>, PersistenceError> {
    let card_num = pic_x_to_string("card_num", card.expose())?;
    let auth_ts = storable_timestamp(auth_ts)?;
    // The key text is cast to `bpchar`: `sqlx` binds a `String` as `text`, against which the
    // `CHAR(n)` column compares as `text`, its trailing spaces dropped, so a key that ends in
    // spaces would find no row and the primary key could not serve the statement.
    let row = sqlx::query_as!(
        AuthFraudRow,
        "SELECT card_num, auth_ts, auth_type, card_expiry_date, message_type, message_source,
                auth_id_code, auth_resp_code, auth_resp_reason, processing_code, transaction_amt,
                approved_amt, merchant_catagory_code, acqr_country_code, pos_entry_mode,
                merchant_id, merchant_name, merchant_city, merchant_state, merchant_zip,
                transaction_id, match_status, auth_fraud, fraud_rpt_date, acct_id, cust_id
         FROM auth_fraud
         WHERE card_num = $1::bpchar AND auth_ts = $2",
        card_num,
        auth_ts,
    )
    .fetch_optional(executor)
    .await?;
    row.map(AuthFraudRow::into_record).transpose()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::{NaiveDate, NaiveDateTime, Timelike};
    use copybook::{PicU, PicX};
    use domain::money::Money;
    use domain::online::copaus2c::{AuthFraudRow as FraudRecord, FraudAction};
    use domain::sensitive::CardNumber;
    use rust_decimal::Decimal;

    use super::AuthFraudRepository;
    use crate::PgStore;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Not a fixture card number: no error may repeat any part of it.
    const CARD: &[u8; 16] = b"9876543210987654";

    /// Another card, under which no row is ever written.
    const OTHER_CARD: &[u8; 16] = b"1234567812345678";

    /// The two index names that can report the duplicate key: the primary key and `xauthfrd`.
    const KEY_INDEXES: [&str; 2] = ["auth_fraud_pkey", "xauthfrd"];

    fn day(year: i32, month: u32, date: u32) -> Result<NaiveDate, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(year, month, date).ok_or_else(|| "invalid test date".into())
    }

    /// A key as the domain builds one: whole milliseconds (COPAUS2C-004).
    fn auth_ts() -> Result<NaiveDateTime, Box<dyn StdError>> {
        day(2022, 1, 15)?
            .and_hms_micro_opt(10, 30, 45, 123_000)
            .ok_or_else(|| "invalid test timestamp".into())
    }

    /// A row as `copaus2c::fraud_row` builds one for a fraud report, every field distinct.
    fn record() -> Result<FraudRecord, Box<dyn StdError>> {
        Ok(FraudRecord {
            card_num: CardNumber::new(PicX::from_array(*CARD)),
            auth_ts: auth_ts()?,
            auth_type: PicX::from_array(*b"0100"),
            card_expiry_date: PicX::from_array(*b"0925"),
            message_type: PicX::from_array(*b"0100  "),
            message_source: PicX::from_array(*b"POS   "),
            auth_id_code: PicX::from_array(*b"123456"),
            auth_resp_code: PicX::from_array(*b"00"),
            auth_resp_reason: PicX::from_array(*b"0000"),
            processing_code: PicX::from_array(*b"004000"),
            transaction_amt: Money::new("1234.56".parse::<Decimal>()?)?,
            approved_amt: Money::new("-0.01".parse::<Decimal>()?)?,
            merchant_catagory_code: PicX::from_array(*b"5411"),
            acqr_country_code: PicX::from_array(*b"840"),
            pos_entry_mode: PicU::new(5)?,
            merchant_id: PicX::from_array(*b"MERCHANT0000001"),
            merchant_name: PicX::from_array(*b"CORNER STORE          "),
            merchant_city: PicX::from_array(*b"SPRINGFIELD  "),
            merchant_state: PicX::from_array(*b"IL"),
            merchant_zip: PicX::from_array(*b"62701    "),
            transaction_id: PicX::from_array(*b"TXN000000000001"),
            match_status: PicX::from_array(*b"P"),
            auth_fraud: FraudAction::Report.flag(),
            acct_id: PicU::new(99_999_999_999)?,
            cust_id: PicU::new(999_999_999)?,
        })
    }

    /// Cardholder Data Redaction: an error names its column or constraint, never the card number.
    fn assert_no_card_digits(error: &PersistenceError) {
        for text in [error.to_string(), format!("{error:?}")] {
            for piece in ["9876", "5432", "3210", "7654"] {
                assert!(!text.contains(piece), "{text}");
            }
        }
    }

    /// The insert stores every column and the bound report date; a second insert of the key is
    /// the `-803` duplicate and changes nothing; the update sets only the flag and the report
    /// date of the row with the key, and a key without a row updates nothing.
    // audit: COPAUS2C-007
    // audit: COPAUS2C-008
    // audit: COPAUS2C-011
    // audit: COPAUS2C-013
    #[tokio::test]
    async fn insert_duplicate_update_and_read_follow_copaus2c() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let original = record()?;
        let card = CardNumber::new(PicX::from_array(*CARD));
        let reported = day(2022, 7, 18)?;

        assert_eq!(store.find_by_id(&card, original.auth_ts).await?, None);
        store.insert(&original, reported).await?;
        assert_eq!(
            store.find_by_id(&card, original.auth_ts).await?,
            Some((original.clone(), Some(reported)))
        );

        let mut again = original.clone();
        again.auth_fraud = FraudAction::Remove.flag();
        let duplicate = store.insert(&again, day(2022, 7, 19)?).await;
        let Err(error) = duplicate else {
            return Err("the second insert of one key must fail".into());
        };
        assert!(error.is_duplicate(), "{error:?}");
        assert!(
            error
                .constraint()
                .is_some_and(|name| KEY_INDEXES.contains(&name)),
            "{error:?}"
        );
        assert_no_card_digits(&error);
        assert_eq!(
            store.find_by_id(&card, original.auth_ts).await?,
            Some((original.clone(), Some(reported)))
        );

        let removed = day(2022, 7, 20)?;
        let flag = FraudAction::Remove.flag();
        assert!(
            store
                .update_flag(&card, original.auth_ts, &flag, removed)
                .await?
        );
        let expected = FraudRecord {
            auth_fraud: flag.clone(),
            ..original.clone()
        };
        assert_eq!(
            store.find_by_id(&card, original.auth_ts).await?,
            Some((expected, Some(removed)))
        );

        let other = CardNumber::new(PicX::from_array(*OTHER_CARD));
        let later = day(2022, 1, 16)?
            .and_hms_micro_opt(10, 30, 45, 123_000)
            .ok_or("invalid test timestamp")?;
        for (key_card, key_ts) in [(&other, original.auth_ts), (&card, later)] {
            assert!(!store.update_flag(key_card, key_ts, &flag, removed).await?);
            assert_eq!(store.find_by_id(key_card, key_ts).await?, None);
        }
        Ok(())
    }

    /// On a transaction the duplicate runs inside a savepoint, so the update that follows runs in
    /// the same unit of work; without the savepoint the duplicate aborts the transaction.
    // audit: COPAUS2C-008
    // audit: COPAUS2C-012
    #[tokio::test]
    async fn a_duplicate_inside_a_savepoint_leaves_the_transaction_usable() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let original = record()?;
        let card = CardNumber::new(PicX::from_array(*CARD));
        let flag = FraudAction::Remove.flag();
        let (first, second) = (day(2022, 7, 18)?, day(2022, 7, 19)?);

        let mut tx = store.begin().await?;
        let mut savepoint = tx.savepoint().await?;
        savepoint.insert(&original, first).await?;
        savepoint.commit().await?;

        let mut savepoint = tx.savepoint().await?;
        let duplicate = savepoint.insert(&original, second).await;
        assert!(
            duplicate
                .as_ref()
                .is_err_and(PersistenceError::is_duplicate)
        );
        savepoint.rollback().await?;
        assert!(
            tx.update_flag(&card, original.auth_ts, &flag, second)
                .await?
        );
        tx.commit().await?;

        let expected = FraudRecord {
            auth_fraud: flag.clone(),
            ..original.clone()
        };
        let mut reader = store.clone();
        assert_eq!(
            reader.find_by_id(&card, original.auth_ts).await?,
            Some((expected, Some(second)))
        );

        let mut tx = store.begin().await?;
        let duplicate = tx.insert(&original, second).await;
        assert!(
            duplicate
                .as_ref()
                .is_err_and(PersistenceError::is_duplicate)
        );
        let aborted = tx.update_flag(&card, original.auth_ts, &flag, second).await;
        assert_eq!(
            aborted.map_err(|error| error.sqlstate().map(str::to_owned)),
            Err(Some("25P02".to_owned()))
        );
        tx.rollback().await?;
        Ok(())
    }

    /// A key finer than a microsecond names no `TIMESTAMP(6)` instant: every method refuses it
    /// before a statement runs, and nothing is written.
    // audit: COPAUS2C-004
    #[tokio::test]
    async fn a_key_the_column_cannot_hold_is_refused_by_every_method() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let card = CardNumber::new(PicX::from_array(*CARD));
        let reported = day(2022, 7, 18)?;
        let finer = day(2022, 1, 15)?
            .and_hms_nano_opt(10, 30, 45, 123_000_001)
            .ok_or("invalid test timestamp")?;
        let refused = Err(PersistenceError::decode("auth_ts", DecodeKind::OutOfRange));

        let mut original = record()?;
        original.auth_ts = finer;
        assert_eq!(store.insert(&original, reported).await, refused);
        let flag = FraudAction::Report.flag();
        assert_eq!(
            store
                .update_flag(&card, finer, &flag, reported)
                .await
                .map(drop),
            refused
        );
        assert_eq!(store.find_by_id(&card, finer).await.map(drop), refused);

        let whole = finer
            .with_nanosecond(123_000_000)
            .ok_or("invalid nanosecond")?;
        assert_eq!(store.find_by_id(&card, whole).await?, None);
        Ok(())
    }
}
