//! The transaction category balances: VSAM `TCATBALF` (`CVTRA01Y`
//! `TRAN-CAT-BAL-RECORD`, `RECLN` 50, `KEYS(17 0)` [app/jcl/TCATBALF.jcl:L36-41]),
//! table `transaction_category_balances`. [`TranCatBalanceStore`] on
//! [`PgStore`].
//!
//! | Access | COBOL | Method |
//! |---|---|---|
//! | Random `READ` by key; status 23 creates the record | `CBTRN02C` `2700-UPDATE-TCATBAL` (P-CBTRN02C-015) | `read_tcatbal` |
//! | `WRITE` of a new record | `CBTRN02C` `2700-A-CREATE-TCATBAL-REC` (P-CBTRN02C-016) | `write_tcatbal` |
//! | `REWRITE` of an existing record | `CBTRN02C` `2700-B-UPDATE-TCATBAL-REC` (P-CBTRN02C-017) | `rewrite_tcatbal` |
//! | Sequential `READ` in key order | `CBACT04C` `1000-TCATBALF-GET-NEXT` (P-CBACT04C-182) | `tcatbals_after` |
//!
//! # Columns
//!
//! One column per `CVTRA01Y` field, as the reviewed section A rows of
//! `docs/parity/parity-oracle-audit.md` fix them; the `FILLER X(22)`
//! (P-CVTRA01Y-005) is not stored.
//!
//! | Column | Field | PIC | Domain type | Audit row |
//! |---|---|---|---|---|
//! | `trancat_acct_id` `BIGINT` | `TRANCAT-ACCT-ID` | `9(11)` | `AccountId` | P-CVTRA01Y-001 |
//! | `trancat_type_cd` `VARCHAR(2) COLLATE "C"` | `TRANCAT-TYPE-CD` | `X(02)` | `TranTypeCode` | P-CVTRA01Y-002 |
//! | `trancat_cd` `INTEGER` | `TRANCAT-CD` | `9(04)` | `TranCategoryCode` | P-CVTRA01Y-003 |
//! | `tran_cat_bal` `NUMERIC(11,2)` | `TRAN-CAT-BAL` | `S9(09)V99` | `Money<9>` | P-CVTRA01Y-004 |
//!
//! # Key order
//!
//! The KSDS key is the 17 bytes `TRANCAT-ACCT-ID 9(11)`, `TRANCAT-TYPE-CD
//! X(02)`, `TRANCAT-CD 9(04)`, compared byte by byte, which is the derived
//! `Ord` of [`TranCatBalanceKey`]. The two zero-padded digit fields sort by
//! their numeric value. The type code sorts by its two padded bytes:
//!
//! - `COLLATE "C"` compares byte values instead of the database's linguistic
//!   order, which would put `a` before `B`.
//! - The column holds the code right-trimmed, and a trimmed code compares as a
//!   prefix of a longer one, so `1` would sort before `1` followed by a
//!   control character. The padded bytes put the control character, below the
//!   space, first. Every ordering comparison therefore pads the stored code
//!   back to its two bytes with `rpad`, which restores the exact `X(02)` value
//!   for every character the column admits (U+0001 to U+00FF, whose UTF-8
//!   bytes keep their code point order). Equality needs no padding: one COBOL
//!   key has exactly one stored form.
//!
//! The index `transaction_category_balances_padded_key`
//! (`migrations/0003_transactions.sql`) holds exactly these three key
//! expressions: `trancat_acct_id`, `rpad(trancat_type_cd, 2) COLLATE "C"` and
//! `trancat_cd`. The browse orders by them, and the browse after a key bounds
//! them with one row comparison,
//! `(trancat_acct_id, rpad(trancat_type_cd, 2) COLLATE "C", trancat_cd) >
//! ($1, rpad($2, 2) COLLATE "C", $3)`. A row comparison is lexicographic and
//! strict, so it selects exactly the keys greater than the one given, and the
//! planner uses it as the index condition: each page starts at the key and
//! reads the next records in index order, with no filter and no sort. Inside
//! the row the type codes compare padded for equality too, which matches the
//! raw equality: both sides are right-trimmed, and padding two right-trimmed
//! codes of at most two characters gives equal values only for equal codes.
//!
//! # Writes
//!
//! The write and the rewrite each run under their own savepoint through
//! `crate::tx::savepoint` and `crate::tx::settle`. `TCATBALF` is
//! `RECOVERY(NONE)` [app/csd/CARDDEMO.CSD:L1-96]: POSTTRAN keeps a balance
//! write in place when a later write of the same record fails, and commits
//! each record on its own. The reads run under their own savepoints too, so a
//! failed read leaves the transaction usable as well.
//!
//! No arithmetic happens here: `rules::posting` adds `DALYTRAN-AMT` to the
//! balance (section C) and the store writes the `Money<9>` it is given.

use std::borrow::Cow;

use carddemo_domain::entities::{TranCatBalance, TranCatBalanceKey};
use carddemo_domain::ports::{PortError, PortResult, TranCatBalanceStore};
use rust_decimal::Decimal;

use super::{account_id, expect_affected, limit_i64, money, tran_category_code, tran_type_code};
use crate::PersistenceError;
use crate::tx::{PgStore, savepoint, settle};

/// The table, as [`PersistenceError::Corrupt`] names it.
const TABLE: &str = "transaction_category_balances";

/// One row of `transaction_category_balances` as `query_as!` reads it.
struct TcatbalRow {
    trancat_acct_id: i64,
    trancat_type_cd: String,
    trancat_cd: i32,
    tran_cat_bal: Decimal,
}

/// The `CVTRA01Y` record rebuilt from its row, each column held to its PIC
/// clause (P-CVTRA01Y-001 to P-CVTRA01Y-004).
impl TryFrom<TcatbalRow> for TranCatBalance {
    type Error = PersistenceError;

    fn try_from(row: TcatbalRow) -> Result<Self, Self::Error> {
        Ok(Self {
            key: TranCatBalanceKey {
                trancat_acct_id: account_id(row.trancat_acct_id, TABLE, "trancat_acct_id")?,
                trancat_type_cd: tran_type_code(&row.trancat_type_cd, TABLE, "trancat_type_cd")?,
                trancat_cd: tran_category_code(row.trancat_cd, TABLE, "trancat_cd")?,
            },
            tran_cat_bal: money::<9>(row.tran_cat_bal, TABLE, "tran_cat_bal")?,
        })
    }
}

/// The key as its three columns bind it: the account as `BIGINT`, the type
/// code right-trimmed and the category as `INTEGER`.
fn key_binds(key: &TranCatBalanceKey) -> (i64, Cow<'_, str>, i32) {
    (
        key.trancat_acct_id.to_i64(),
        key.trancat_type_cd.trimmed(),
        i32::from(key.trancat_cd.get()),
    )
}

/// Each stored row rebuilt as its record.
fn records(rows: Vec<TcatbalRow>) -> PortResult<Vec<TranCatBalance>> {
    rows.into_iter()
        .map(|row| TranCatBalance::try_from(row).map_err(PortError::from))
        .collect()
}

impl TranCatBalanceStore for PgStore<'_, '_> {
    /// CBTRN02C 2700-UPDATE-TCATBAL — P-CBTRN02C-015
    /// [app/cbl/CBTRN02C.cbl:L467-499]: `READ TCATBAL-FILE` by
    /// `FD-TRAN-CAT-KEY`. A missing key is [`PortError::NotFound`], the
    /// `INVALID KEY` (status 23) on which the rule creates the record.
    async fn read_tcatbal(&mut self, key: TranCatBalanceKey) -> PortResult<TranCatBalance> {
        let (acct_id, type_cd, cat_cd) = key_binds(&key);
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            TcatbalRow,
            "SELECT trancat_acct_id, trancat_type_cd, trancat_cd, tran_cat_bal \
             FROM transaction_category_balances \
             WHERE trancat_acct_id = $1 AND trancat_type_cd = $2 AND trancat_cd = $3",
            acct_id,
            &*type_cd,
            cat_cd
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?.ok_or(PortError::NotFound)?;
        Ok(TranCatBalance::try_from(row)?)
    }

    /// CBTRN02C 2700-A-CREATE-TCATBAL-REC — P-CBTRN02C-016
    /// [app/cbl/CBTRN02C.cbl:L503-524]: `WRITE FD-TRAN-CAT-BAL-RECORD`, one
    /// legacy write under its own savepoint. A key already present is
    /// [`PortError::Duplicate`] (status 22).
    async fn write_tcatbal(&mut self, rec: TranCatBalance) -> PortResult<()> {
        let (acct_id, type_cd, cat_cd) = key_binds(&rec.key);
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "INSERT INTO transaction_category_balances \
             (trancat_acct_id, trancat_type_cd, trancat_cd, tran_cat_bal) \
             VALUES ($1, $2, $3, $4)",
            acct_id,
            &*type_cd,
            cat_cd,
            rec.tran_cat_bal.to_decimal()
        )
        .execute(&mut *sp)
        .await;
        settle(sp, result).await?;
        Ok(())
    }

    /// CBTRN02C 2700-B-UPDATE-TCATBAL-REC — P-CBTRN02C-017
    /// [app/cbl/CBTRN02C.cbl:L526-542]: `REWRITE FD-TRAN-CAT-BAL-RECORD`, one
    /// legacy write under its own savepoint. Only the balance changes; a key
    /// that names no record is [`PortError::NotFound`].
    async fn rewrite_tcatbal(&mut self, rec: TranCatBalance) -> PortResult<()> {
        let (acct_id, type_cd, cat_cd) = key_binds(&rec.key);
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "UPDATE transaction_category_balances SET tran_cat_bal = $4 \
             WHERE trancat_acct_id = $1 AND trancat_type_cd = $2 AND trancat_cd = $3",
            acct_id,
            &*type_cd,
            cat_cd,
            rec.tran_cat_bal.to_decimal()
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }

    /// CBACT04C 1000-TCATBALF-GET-NEXT — P-CBACT04C-182
    /// [app/cbl/CBACT04C.cbl:L325-348]: the next `READ TCATBAL-FILE` calls of
    /// the sequential pass, up to `limit` records after `after` in key order.
    /// The end of the file (status 10) is an empty page.
    async fn tcatbals_after(
        &mut self,
        after: Option<TranCatBalanceKey>,
        limit: u16,
    ) -> PortResult<Vec<TranCatBalance>> {
        let limit = limit_i64(limit)?;
        // `rpad(..) COLLATE "C"` rather than the bare column: the order is the
        // padded X(02) byte order of the key, as "Key order" above explains.
        // The bound after a key is one row comparison over the same three
        // expressions, the key of `transaction_category_balances_padded_key`,
        // so the index seeks to the key instead of filtering the account.
        let mut sp = savepoint(self.tx).await?;
        let result = match after {
            None => {
                sqlx::query_as!(
                    TcatbalRow,
                    r#"SELECT trancat_acct_id, trancat_type_cd, trancat_cd, tran_cat_bal
                       FROM transaction_category_balances
                       ORDER BY trancat_acct_id, rpad(trancat_type_cd, 2) COLLATE "C", trancat_cd
                       LIMIT $1"#,
                    limit
                )
                .fetch_all(&mut *sp)
                .await
            }
            Some(key) => {
                let (acct_id, type_cd, cat_cd) = key_binds(&key);
                sqlx::query_as!(
                    TcatbalRow,
                    r#"SELECT trancat_acct_id, trancat_type_cd, trancat_cd, tran_cat_bal
                       FROM transaction_category_balances
                       WHERE (trancat_acct_id, rpad(trancat_type_cd, 2) COLLATE "C", trancat_cd)
                             > ($1, rpad($2, 2) COLLATE "C", $3)
                       ORDER BY trancat_acct_id, rpad(trancat_type_cd, 2) COLLATE "C", trancat_cd
                       LIMIT $4"#,
                    acct_id,
                    &*type_cd,
                    cat_cd,
                    limit
                )
                .fetch_all(&mut *sp)
                .await
            }
        };
        records(settle(sp, result).await?)
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::{TranCatBalance, TranCatBalanceKey};
    use carddemo_domain::numeric::{
        AccountId, Money, NumericError, TranCategoryCode, TranTypeCode,
    };
    use rust_decimal::Decimal;

    use super::{TABLE, TcatbalRow, key_binds};
    use crate::PersistenceError;

    /// The `NumericError` texts the domain constructors report.
    const OUT_OF_RANGE: &str = "value is outside the range of the field";
    const NEGATIVE: &str = "value must not be negative";
    const TOO_MANY_DECIMALS: &str = "value has more decimal places than the field allows";

    /// The row of the first `tcatbal.txt` record with the given type code and
    /// balance.
    fn row(type_cd: &str, tran_cat_bal: Decimal) -> TcatbalRow {
        TcatbalRow {
            trancat_acct_id: 1,
            trancat_type_cd: type_cd.to_owned(),
            trancat_cd: 1,
            tran_cat_bal,
        }
    }

    fn key(acct_id: u64, type_cd: &str, cat_cd: u16) -> Result<TranCatBalanceKey, NumericError> {
        Ok(TranCatBalanceKey {
            trancat_acct_id: AccountId::new(acct_id)?,
            trancat_type_cd: TranTypeCode::move_from(type_cd),
            trancat_cd: TranCategoryCode::new(cat_cd)?,
        })
    }

    #[test]
    fn a_stored_row_becomes_its_record_with_the_type_code_padded() -> Result<(), NumericError> {
        let cases = [
            (row("01", Decimal::new(-7_077, 2)), "01", -7_077),
            (row("1", Decimal::new(116_487, 2)), "1 ", 116_487),
            (row("", Decimal::ZERO), "  ", 0),
        ];
        for (stored, type_cd, cents) in cases {
            let expected = TranCatBalance {
                key: key(1, type_cd, 1)?,
                tran_cat_bal: Money::from_cents(cents)?,
            };
            match TranCatBalance::try_from(stored) {
                Ok(record) => {
                    assert_eq!(record, expected, "{type_cd:?}");
                    assert_eq!(record.key.trancat_type_cd.as_bytes(), type_cd.as_bytes());
                }
                Err(err) => panic!("{type_cd:?}: {err}"),
            }
        }
        Ok(())
    }

    #[test]
    fn a_value_outside_its_picture_is_corrupt_naming_its_column() {
        let cases = [
            (
                TcatbalRow {
                    trancat_acct_id: -1,
                    ..row("01", Decimal::ZERO)
                },
                "trancat_acct_id",
                NEGATIVE.to_owned(),
            ),
            (
                TcatbalRow {
                    trancat_acct_id: 100_000_000_000,
                    ..row("01", Decimal::ZERO)
                },
                "trancat_acct_id",
                OUT_OF_RANGE.to_owned(),
            ),
            (
                row("011", Decimal::ZERO),
                "trancat_type_cd",
                "expected at most 2 characters, got 3".to_owned(),
            ),
            (
                TcatbalRow {
                    trancat_cd: 10_000,
                    ..row("01", Decimal::ZERO)
                },
                "trancat_cd",
                OUT_OF_RANGE.to_owned(),
            ),
            (
                row("01", Decimal::new(100_000_000_000, 2)),
                "tran_cat_bal",
                OUT_OF_RANGE.to_owned(),
            ),
            (
                row("01", Decimal::new(1_001, 3)),
                "tran_cat_bal",
                TOO_MANY_DECIMALS.to_owned(),
            ),
        ];
        for (stored, expected_column, expected_reason) in cases {
            match TranCatBalance::try_from(stored) {
                Err(PersistenceError::Corrupt {
                    table,
                    column,
                    reason,
                }) => {
                    assert_eq!((table, column), (TABLE, expected_column));
                    assert_eq!(reason, expected_reason, "{expected_column}");
                }
                other => panic!("{expected_column}: expected Corrupt, got {other:?}"),
            }
        }
    }

    #[test]
    fn keys_bind_as_their_columns_store_them() -> Result<(), NumericError> {
        let cases = [
            (key(1, "01", 1)?, (1, "01", 1)),
            (
                key(99_999_999_999, "1", 9_999)?,
                (99_999_999_999, "1", 9_999),
            ),
            (key(0, "", 0)?, (0, "", 0)),
        ];
        for (input, (acct_id, type_cd, cat_cd)) in cases {
            let (bound_acct, bound_type, bound_cat) = key_binds(&input);
            assert_eq!(
                (bound_acct, &*bound_type, bound_cat),
                (acct_id, type_cd, cat_cd)
            );
        }
        Ok(())
    }
}
