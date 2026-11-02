//! The serialized `TRAN-ID` allocation of `COTRN02C` (CT02) and `COBIL00C`
//! (CB00): the highest live key plus one.
//!
//! Both programs take the key the same way: `MOVE HIGH-VALUES TO TRAN-ID`,
//! `STARTBR`, one `READPREV`, `ENDBR`, then `ADD 1` in a `PIC 9(16)` working
//! field [app/cbl/COTRN02C.cbl:L442-451] [app/cbl/COBIL00C.cbl:L212-219]. The
//! replacement reads `max(tran_id)` instead of browsing, and
//! [`TranId::next`] performs the `ADD 1`. [`next_tran_id`] is the only
//! implementation; no consumer allocates a TRAN-ID on its own.
//!
//! # Serialization
//!
//! Two units of work that both read the highest key before either inserts
//! would allocate the same TRAN-ID, and the later insert would fail with the
//! duplicate-key outcome (`DUPREC`, "Tran ID already exist..."). The
//! allocation therefore first takes the transaction-scoped advisory lock
//! [`TRAN_ID_LOCK_KEY`] with `pg_advisory_xact_lock`. The lock is held until
//! the caller's transaction (its `UnitOfWork`) commits or rolls back, so the
//! key read stays the highest one until the new row is written, and concurrent
//! adds receive consecutive keys. There is no unlock call: dropping an
//! uncommitted transaction rolls it back and releases the lock with it.
//!
//! The lock and the read are two statements on purpose. Under `READ
//! COMMITTED`, the `PostgreSQL` default the unit of work runs at, each
//! statement takes its own snapshot, so the read issued after the lock is
//! granted sees the row the previous holder committed. A single statement
//! would take its snapshot before waiting for the lock and could read a stale
//! maximum.
//!
//! # The live master
//!
//! The read goes through the view `transaction_master`, not the base table:
//! only live rows form the `TRANSACT` KSDS. Rows that `batch tranbkp` takes out
//! of the live master stay in the table for the reward accruals that reference
//! them (E-09) and never count towards the highest key, so a TRAN-ID held only
//! by such a row may be issued again.
//!
//! The `tran_id` column is `VARCHAR(16) COLLATE "C"`
//! (`migrations/0003_transactions.sql`), so `max` compares byte values: the key
//! order the `READPREV` from `HIGH-VALUES` follows under ASCII collation.

use carddemo_domain::numeric::{NumericError, TranId};
use carddemo_domain::ports::PortError;
use sqlx::{Postgres, Transaction};

use crate::PersistenceError;

/// The advisory lock key that serializes TRAN-ID allocation.
///
/// The eight ASCII bytes `CDTRANID` read as a big-endian `i64`
/// (`0x4344_5452_414E_4944`). The value is fixed: every process that allocates
/// or must not interleave with an allocation takes the same key.
pub const TRAN_ID_LOCK_KEY: i64 = i64::from_be_bytes(*b"CDTRANID");

/// The view the allocation reads, as [`PersistenceError::Corrupt`] names it.
const MASTER_VIEW: &str = "transaction_master";

/// The key column the allocation reads.
const KEY_COLUMN: &str = "tran_id";

/// Allocates the next TRAN-ID inside the caller's transaction.
///
/// COTRN02C ADD-TRANSACTION — P-COTRN02C-020 [app/cbl/COTRN02C.cbl:L442-451];
/// COBIL00C PROCESS-ENTER-KEY — P-COBIL00C-012 [app/cbl/COBIL00C.cbl:L210-219].
/// Takes the advisory lock [`TRAN_ID_LOCK_KEY`] for the rest of `tx`, reads
/// the highest live `tran_id` and returns it plus one, 16 digits with leading
/// zeros.
///
/// An empty live master, as `batch tranbkp` leaves it, is the `STARTBR`
/// `NOTFND` path of both programs: COTRN02C STARTBR-TRANSACT-FILE —
/// P-COTRN02C-021 [app/cbl/COTRN02C.cbl:L642-668]; COBIL00C
/// STARTBR-TRANSACT-FILE — P-COBIL00C-013 [app/cbl/COBIL00C.cbl:L441-467]. No
/// key is allocated and nothing is added (E-27). The `READPREV` `ENDFILE` path
/// that would give `0000000000000001` (P-COTRN02C-022, P-COBIL00C-014) is
/// unreachable after a successful `STARTBR`, so it has no counterpart here.
///
/// The function only reads, so it needs no savepoint; the caller commits or
/// rolls back the transaction that holds the lock.
///
/// # Errors
///
/// - [`PersistenceError::Port`] with [`PortError::NotFound`] when the live
///   master is empty. The rules turn it into the program's own message:
///   "Transaction ID NOT found..." for CT02 (V-COTRN02C-029) and "Unable to
///   lookup Transaction..." for CB00 (V-COBIL00C-011).
/// - [`PersistenceError::Sqlx`] when the lock or the read fails, the `WHEN
///   OTHER` branch of the browse (V-COTRN02C-030, V-COTRN02C-031,
///   V-COBIL00C-012, V-COBIL00C-013).
/// - [`PersistenceError::Corrupt`] when the highest stored key is not 16 ASCII
///   digits, the only keys [`TranId::next`] can add one to.
pub async fn next_tran_id(tx: &mut Transaction<'_, Postgres>) -> Result<TranId, PersistenceError> {
    sqlx::query_scalar!(
        r#"SELECT 1 AS "locked!" FROM pg_advisory_xact_lock($1)"#,
        TRAN_ID_LOCK_KEY
    )
    .fetch_one(&mut **tx)
    .await?;
    let highest = sqlx::query_scalar!("SELECT max(tran_id) AS max_tran_id FROM transaction_master")
        .fetch_one(&mut **tx)
        .await?;
    next_after(highest.as_deref())
}

/// The TRAN-ID that follows `highest`, the right-trimmed text of the highest
/// live key, or [`PortError::NotFound`] when there is none.
fn next_after(highest: Option<&str>) -> Result<TranId, PersistenceError> {
    let Some(stored) = highest else {
        return Err(PersistenceError::Port(PortError::NotFound));
    };
    TranId::from_trimmed(stored)
        .and_then(TranId::next)
        .map_err(|err| corrupt_key(stored, &err))
}

/// The error for a highest key that cannot be read back or added one to. The
/// key is named, since it is the row's key and not its contents.
fn corrupt_key(stored: &str, err: &NumericError) -> PersistenceError {
    PersistenceError::Corrupt {
        table: MASTER_VIEW,
        column: KEY_COLUMN,
        reason: format!("highest key {stored:?}: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::numeric::TranId;
    use carddemo_domain::ports::PortError;

    use super::{PersistenceError, TRAN_ID_LOCK_KEY, next_after};

    #[test]
    fn the_lock_key_is_the_ascii_name() {
        assert_eq!(TRAN_ID_LOCK_KEY.to_be_bytes(), *b"CDTRANID");
        assert_eq!(TRAN_ID_LOCK_KEY, 0x4344_5452_414E_4944);
    }

    #[test]
    fn the_next_key_is_the_highest_plus_one() -> Result<(), PersistenceError> {
        let cases = [
            ("0000000000000300", "0000000000000301"),
            ("0000000000000000", "0000000000000001"),
            ("0000000000000999", "0000000000001000"),
            ("9999999999999998", "9999999999999999"),
            ("9999999999999999", "0000000000000000"),
        ];
        for (highest, expected) in cases {
            assert_eq!(
                next_after(Some(highest))?,
                TranId::move_from(expected),
                "{highest}"
            );
        }
        Ok(())
    }

    #[test]
    fn an_empty_live_master_is_not_found() {
        assert!(matches!(
            next_after(None),
            Err(PersistenceError::Port(PortError::NotFound))
        ));
    }

    #[test]
    fn a_key_that_is_not_sixteen_digits_is_corrupt() {
        let cases = [
            // Shorter than the field: padded with spaces, which are no digits.
            (
                "000000000000030",
                r#"highest key "000000000000030": value is not numeric"#,
            ),
            (
                "ABCDEFGHIJKLMNOP",
                r#"highest key "ABCDEFGHIJKLMNOP": value is not numeric"#,
            ),
            ("", r#"highest key "": value is not numeric"#),
            (
                "00000000000000001",
                r#"highest key "00000000000000001": expected at most 16 characters, got 17"#,
            ),
            (
                "000000000000000\u{20AC}",
                "highest key \"000000000000000\u{20AC}\": value has a character outside \
                 ISO 8859-1",
            ),
        ];
        for (highest, expected) in cases {
            match next_after(Some(highest)) {
                Err(PersistenceError::Corrupt {
                    table,
                    column,
                    reason,
                }) => {
                    assert_eq!((table, column), ("transaction_master", "tran_id"));
                    assert_eq!(reason, expected);
                }
                other => panic!("{highest:?}: expected Corrupt, got {other:?}"),
            }
        }
    }
}
