//! The CP00 error log `cssl_error_log` (migration 0005), the replacement for the CICS transient
//! data queue `CSSL`.
//!
//! `COPAUA0C` paragraph `9500-LOG-ERROR` stamps `ERROR-LOG-RECORD` (`CCPAUERY.cpy:19-40`) with
//! the date, time, transaction ID and program name and writes it with
//! `EXEC CICS WRITEQ TD QUEUE('CSSL')` (`COPAUA0C.cbl:983-1011`, the write at 1001-1006;
//! COPAUA0C-110). Every log site of the program reaches it, among them `M004`
//! `FAILED TO PUT ON REPLY MQ` (`COPAUA0C.cbl:767-779`; COPAUA0C-117), `I003`
//! `IMS UPDATE SUMRY FAILED` (`COPAUA0C.cbl:840-846`; COPAUA0C-119) and `I004`
//! `IMS INSERT DETL FAILED` (`COPAUA0C.cbl:925-931`; COPAUA0C-120). Each write is one
//! [`CsslErrorLogRepository::insert`], the record converted by codec B
//! ([`crate::rows::cssl_error_log`]) with every `CHAR(n)` column's padding kept.
//!
//! A transient data queue is appended to and drained, never read by key, so the table adds the
//! surrogate `id` that the database assigns (CCPAUERY-012), and the one read,
//! [`CsslErrorLogRepository::find_all`], returns the whole log in write order.
//!
//! # Its own transaction (AAP 0.5.2)
//!
//! A `CSSL` write is not part of the CICS unit of work: the record stands whether the
//! authorization it reports commits or not. The CP00 worker therefore writes through
//! [`PgStore`], whose autocommit insert commits at once in a transaction of its own, and never
//! through the authorization [`PgTx`] that holds the i10 lock on the account's summary row: a
//! row written there would vanish with the rollback of an `M004` or `I003` failure, and the
//! `I004` row would stand or fall with the summary update it reports. The [`PgTx`]
//! implementation exists for tests and for symmetry with the other repositories.
//!
//! # Event key (Cardholder Data Redaction, E32)
//!
//! `COPAUA0C` moves full card numbers into `ERR-EVENT-KEY` (`COPAUA0C.cbl:428,499,511,777,845,
//! 930`). This table is CP00's log record, so the rule reaches it: a card-number key arrives
//! already redacted by `domain::records::error_log::card_event_key`, as `************5740`
//! followed by four spaces, and is stored and read back verbatim. Nothing here derives, widens
//! or unmasks a key, and nothing here emits a tracing event.
//!
//! `ERR-CODE-1` and `ERR-CODE-2` are stored as given as well; the CICS, MQ and IMS codes the
//! worker puts there are halt H4's concern (COPAUA0C-112), not this module's.

use domain::records::error_log::ErrorLogRecord;

use crate::error::PersistenceError;
use crate::rows::{CsslErrorLogBinds, CsslErrorLogRow};
use crate::{PgStore, PgTx};

/// The CP00 error log, one row per `9500-LOG-ERROR` write to TDQ `CSSL`
/// (`COPAUA0C.cbl:983-1011`; COPAUA0C-110).
///
/// Write it through [`PgStore`], so that each row commits immediately and independently of the
/// authorization it records (AAP 0.5.2): the CP00 worker never writes it through the
/// authorization [`PgTx`] that holds the i10 summary lock, because that transaction rolls back
/// on an `M004` or `I003` failure. The [`PgTx`] implementation exists for tests and for
/// symmetry, and its rows commit or roll back with the caller's transaction.
pub trait CsslErrorLogRepository: Send {
    /// Appends `record` as one row and returns the `id` the database assigned to it, which is
    /// greater than every `id` assigned before it. Every field is stored verbatim, the event key
    /// included, which must already hold a card number only in its redacted form (E32).
    ///
    /// The COBOL writes with `NOHANDLE` (`COPAUA0C.cbl:1001-1006`), so a failed `CSSL` write goes
    /// unnoticed there; here the failure is returned, and the caller decides what follows.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming the column, before any statement runs, when a field
    /// holds a NUL or non-ASCII byte, which no `CHAR(n)` column stores byte for byte;
    /// [`PersistenceError::Unavailable`] when the database cannot be reached; and
    /// [`PersistenceError::Database`] when the server refuses the insert.
    fn insert(
        &mut self,
        record: &ErrorLogRecord,
    ) -> impl Future<Output = Result<i64, PersistenceError>> + Send;

    /// Every row in write order (`id` ascending), each as its `id` and record.
    ///
    /// Unbounded, for tests and diagnostics: no API endpoint lists this table, as no `CardDemo`
    /// transaction reads TDQ `CSSL`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming the column when a stored value does not fit its
    /// picture; [`PersistenceError::Unavailable`] when the database cannot be reached; and
    /// [`PersistenceError::Database`] when the server refuses the read.
    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<(i64, ErrorLogRecord)>, PersistenceError>> + Send;
}

// Each method keeps the trait's exact signature, `Send` included, rather than being an
// `async fn`: it converts the record and takes the executor first, and its async block only
// awaits the statement, as the methods of `PgTx` do.
impl CsslErrorLogRepository for PgStore {
    fn insert(
        &mut self,
        record: &ErrorLogRecord,
    ) -> impl Future<Output = Result<i64, PersistenceError>> + Send {
        let binds = CsslErrorLogBinds::from_record(record);
        let pool = self.pool();
        async move {
            let binds = binds?;
            insert_one(pool, &binds).await
        }
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<(i64, ErrorLogRecord)>, PersistenceError>> + Send {
        let rows = select_all(self.pool());
        async move { into_records(rows.await?) }
    }
}

impl CsslErrorLogRepository for PgTx<'_> {
    fn insert(
        &mut self,
        record: &ErrorLogRecord,
    ) -> impl Future<Output = Result<i64, PersistenceError>> + Send {
        let binds = CsslErrorLogBinds::from_record(record);
        let conn = self.conn();
        async move {
            let binds = binds?;
            insert_one(conn, &binds).await
        }
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<(i64, ErrorLogRecord)>, PersistenceError>> + Send {
        let rows = select_all(self.conn());
        async move { into_records(rows.await?) }
    }
}

/// Inserts one row from `binds`, the eleven `CCPAUERY` columns in migration order, and returns
/// the `id` the `BIGSERIAL` default assigned. The `id` is never bound.
async fn insert_one<'e, E>(executor: E, binds: &CsslErrorLogBinds) -> Result<i64, PersistenceError>
where
    E: sqlx::PgExecutor<'e>,
{
    let id = sqlx::query_scalar!(
        "INSERT INTO cssl_error_log (err_date, err_time, err_application, err_program, \
         err_location, err_level, err_subsystem, err_code_1, err_code_2, err_message, \
         err_event_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
         RETURNING id",
        binds.err_date,
        binds.err_time,
        binds.err_application,
        binds.err_program,
        binds.err_location,
        binds.err_level,
        binds.err_subsystem,
        binds.err_code_1,
        binds.err_code_2,
        binds.err_message,
        binds.err_event_key,
    )
    .fetch_one(executor)
    .await?;
    Ok(id)
}

/// Reads every row, `id` ascending through the primary key.
async fn select_all<'e, E>(executor: E) -> Result<Vec<CsslErrorLogRow>, PersistenceError>
where
    E: sqlx::PgExecutor<'e>,
{
    let rows = sqlx::query_as!(
        CsslErrorLogRow,
        "SELECT id, err_date, err_time, err_application, err_program, err_location, err_level, \
         err_subsystem, err_code_1, err_code_2, err_message, err_event_key \
         FROM cssl_error_log \
         ORDER BY id"
    )
    .fetch_all(executor)
    .await?;
    Ok(rows)
}

/// Converts `rows` back into records, keeping their order; the first row that does not fit its
/// pictures fails the whole read.
fn into_records(
    rows: Vec<CsslErrorLogRow>,
) -> Result<Vec<(i64, ErrorLogRecord)>, PersistenceError> {
    rows.into_iter().map(CsslErrorLogRow::into_record).collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::PicX;
    use domain::records::error_log::{ErrorLevel, ErrorLogRecord, ErrorSubsystem, card_event_key};
    use domain::sensitive::CardNumber;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use super::CsslErrorLogRepository;
    use crate::PgStore;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Runs `test` to completion on a current-thread Tokio runtime built here, with no lint
    /// attribute.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    /// The first cross-reference record's card number (`cardxref.txt:1`).
    const CARD: &str = "0500024453765740";

    /// `ERR-EVENT-KEY` for [`CARD`]: its redacted form, then four spaces (E32).
    const CARD_KEY: [u8; 20] = *b"************5740    ";

    /// A record as `9500-LOG-ERROR` completes it (`COPAUA0C.cbl:983-1011`): one date and time,
    /// `CP00`, `COPAUA0C`, and the location, level, subsystem, message and key of its site.
    fn record(
        location: [u8; 4],
        level: ErrorLevel,
        subsystem: ErrorSubsystem,
        message: &[u8],
        event_key: PicX<20>,
    ) -> ErrorLogRecord {
        ErrorLogRecord {
            date: PicX::from_array(*b"220718"),
            time: PicX::from_array(*b"193005"),
            application: PicX::moved_from(b"CP00"),
            program: PicX::from_array(*b"COPAUA0C"),
            location: PicX::from_array(location),
            level: level.pic(),
            subsystem: subsystem.pic(),
            code_1: PicX::spaces(),
            code_2: PicX::spaces(),
            message: PicX::moved_from(message),
            event_key,
        }
    }

    /// The `M004`, `I003` and `I004` rows for [`CARD`] (COPAUA0C-117, COPAUA0C-119,
    /// COPAUA0C-120), the last with the duplicate-segment status `II` (COPAUA0C-099), and an
    /// `A002` row keyed by an account ID as written (COPAUA0C-115).
    fn log_records() -> Result<[ErrorLogRecord; 4], Box<dyn StdError>> {
        let card_key = card_event_key(&CardNumber::parse(CARD)?);
        assert_eq!(card_key.as_bytes(), &CARD_KEY);
        let critical = ErrorLevel::Critical;
        Ok([
            record(
                *b"M004",
                critical,
                ErrorSubsystem::Mq,
                b"FAILED TO PUT ON REPLY MQ",
                card_key.clone(),
            ),
            record(
                *b"I003",
                critical,
                ErrorSubsystem::Ims,
                b"IMS UPDATE SUMRY FAILED",
                card_key.clone(),
            ),
            ErrorLogRecord {
                code_1: PicX::moved_from(b"II"),
                ..record(
                    *b"I004",
                    critical,
                    ErrorSubsystem::Ims,
                    b"IMS INSERT DETL FAILED",
                    card_key,
                )
            },
            record(
                *b"A002",
                ErrorLevel::Warning,
                ErrorSubsystem::App,
                b"ACCT NOT FOUND IN XREF",
                PicX::moved_from(b"00000000050"),
            ),
        ])
    }

    /// A store whose pool connects lazily to an address nothing listens on, so any statement it
    /// ran would fail as unavailable. The pool's maintenance tasks need a Tokio runtime.
    fn unreachable_store() -> PgStore {
        let options = PgConnectOptions::new()
            .host("127.0.0.1")
            .port(9)
            .username("carddemo")
            .database("carddemo");
        PgStore::new(PgPoolOptions::new().connect_lazy_with(options))
    }

    // audit: COPAUA0C-110
    // audit: CCPAUERY-011
    // audit: CCPAUERY-012
    #[test]
    fn inserts_get_increasing_ids_and_read_back_verbatim_in_write_order() -> TestResult {
        block_on(async {
            let guard = test_support::postgres::start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            assert_eq!(store.find_all().await?, Vec::new());

            let mut written: Vec<(i64, ErrorLogRecord)> = Vec::new();
            for record in log_records()? {
                let id = store.insert(&record).await?;
                if let Some((previous, _)) = written.last() {
                    assert!(id > *previous, "id {id} after {previous}");
                }
                written.push((id, record));
            }
            let read = store.find_all().await?;
            assert_eq!(read, written);

            // A card-number key is stored only as the redacted form it arrived in.
            let keys: Vec<&[u8; 20]> = read
                .iter()
                .map(|(_, row)| row.event_key.as_bytes())
                .collect();
            assert_eq!(
                keys,
                [&CARD_KEY, &CARD_KEY, &CARD_KEY, b"00000000050         "]
            );
            for (_, row) in &read {
                assert!(!row.event_key.to_text().contains(CARD));
            }
            Ok(())
        })
    }

    // audit: COPAUA0C-110
    // audit: COPAUA0C-119
    #[test]
    fn a_store_row_stands_when_a_concurrently_open_transaction_rolls_back() -> TestResult {
        block_on(async {
            let guard = test_support::postgres::start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            let [m004, i003, i004, _] = log_records()?;

            // The authorization transaction is open when the worker logs through the store.
            let mut tx = store.begin().await?;
            let in_tx = tx.insert(&i003).await?;
            let own = store.insert(&m004).await?;
            assert!(own > in_tx);
            assert_eq!(
                tx.find_all().await?,
                [(in_tx, i003.clone()), (own, m004.clone())]
            );
            assert_eq!(store.find_all().await?, [(own, m004.clone())]);

            tx.rollback().await?;
            assert_eq!(store.find_all().await?, [(own, m004.clone())]);

            // A committed transaction keeps its row, and the sequence never goes back: the id
            // the rolled-back insert took is not reused.
            let mut tx = store.begin().await?;
            let committed = tx.insert(&i004).await?;
            tx.commit().await?;
            assert!(committed > own);
            assert_eq!(store.find_all().await?, [(own, m004), (committed, i004)]);
            Ok(())
        })
    }

    // audit: PIC-002
    #[test]
    fn a_byte_no_char_column_stores_fails_before_any_statement() -> TestResult {
        block_on(async {
            let mut store = unreachable_store();
            let [m004, ..] = log_records()?;
            let mut message = *m004.message.as_bytes();
            if let Some(first) = message.first_mut() {
                *first = 0xC3;
            }
            let invalid = ErrorLogRecord {
                message: PicX::from_array(message),
                ..m004
            };
            // Reaching the database would report it unavailable; the conversion fails first.
            assert_eq!(
                store.insert(&invalid).await,
                Err(PersistenceError::decode(
                    "err_message",
                    DecodeKind::InvalidByte
                ))
            );
            Ok(())
        })
    }

    #[test]
    fn a_closed_pool_is_unavailable_to_both_methods() -> TestResult {
        block_on(async {
            let mut store = unreachable_store();
            store.pool().close().await;
            let [m004, ..] = log_records()?;
            assert_eq!(
                store.insert(&m004).await,
                Err(PersistenceError::Unavailable)
            );
            assert_eq!(store.find_all().await, Err(PersistenceError::Unavailable));
            Ok(())
        })
    }
}
