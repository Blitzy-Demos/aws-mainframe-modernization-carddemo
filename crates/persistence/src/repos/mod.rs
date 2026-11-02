//! The port implementations on [`PgStore`](crate::tx::PgStore), one file per
//! table group, and the row conversions they share.
//!
//! | Module | COBOL file or table | `PostgreSQL` table or view |
//! |---|---|---|
//! | [`accounts`] | VSAM `ACCTDAT` (`CVACT01Y`) | `accounts` |
//! | [`cards`] | VSAM `CARDDAT` and its path `CARDAIX` (`CVACT02Y`) | `cards` |
//! | [`customers`] | VSAM `CUSTDAT` (`CVCUS01Y`) | `customers` |
//! | [`xref`] | VSAM `CCXREF` and its path `CXACAIX` (`CVACT03Y`) | `card_xref` |
//! | [`transactions`] | VSAM `TRANSACT` (`CVTRA05Y`) | view `transaction_master`; its table for writes |
//! | [`tcatbal`] | VSAM `TCATBALF` (`CVTRA01Y`) | `transaction_category_balances` |
//! | [`discgrp`] | VSAM `DISCGRP` (`CVTRA02Y`) | `disclosure_groups` |
//! | [`tran_types`] | Db2 `TRANSACTION_TYPE` and `TRANSACTION_TYPE_CATEGORY`; VSAM `TRANTYPE` and `TRANCATG` (`CVTRA03Y`, `CVTRA04Y`) | `transaction_types`, `transaction_categories`; `trantype_file`, `trancatg_file` |
//! | [`users`] | VSAM `USRSEC` (`CSUSR01Y`) | `users` |
//! | [`authorizations`] | IMS `DBPAUTP0` (`CIPAUSMY`, `CIPAUDTY`) and Db2 `AUTHFRDS` | `pending_auth_summaries`, `pending_auth_details`, `auth_fraud` |
//! | [`rewards`] | none: the reward system is new | `reward_rules`, `reward_balances`, `reward_accruals` |
//!
//! # Shape
//!
//! Every file implements the `carddemo_domain::ports` traits of its table group
//! on the single adapter [`PgStore`](crate::tx::PgStore), and adds the inherent
//! batch and admin methods the ports do not declare. The rules take one store
//! that implements several ports, so no file defines an adapter type of its own.
//!
//! # Writes
//!
//! Every `CardDemo` VSAM file is `RECOVERY(NONE)` [app/csd/CARDDEMO.CSD:L1-96]:
//! a completed write survives a later failure in the same task. Each legacy
//! write and each locking read (`SELECT ... FOR UPDATE`) therefore runs under its
//! own savepoint through `crate::tx::savepoint` and `crate::tx::settle`, the one
//! shared unit-of-work component; no file here opens, releases or rolls back a
//! savepoint itself. Plain reads run on the transaction directly.
//!
//! # Transaction reads
//!
//! Every legacy read of the transaction master goes through the view
//! `transaction_master`, which holds only the live rows (E-09). Only
//! [`transactions`] names the underlying table, for its writes.
//!
//! # Row conversions
//!
//! The helpers below turn column values into the domain types of the copybook
//! fields, so each column is held to its PIC clause on the way back in, as the
//! reviewed section A rows of `docs/parity/parity-oracle-audit.md` fix it:
//!
//! - text: `VARCHAR(n)` holds an `X(n)` value right-trimmed (AAP 0.5.3.4), and
//!   [`Alnum`] restores its padding;
//! - money and rates: `NUMERIC` becomes [`Money`], [`InterestRate`] or
//!   [`RewardRate`] only when it fits the picture exactly;
//! - integers: a `SMALLINT`, `INTEGER` or `BIGINT` column widens losslessly to
//!   `i64` and enters through the type's `TryFrom<i64>`;
//! - points: `BIGINT` becomes [`RewardPoints`].
//!
//! A value that does not fit is never cut, rounded or wrapped. It is
//! [`PersistenceError::Corrupt`] naming the table, the column and the domain
//! constructor's reason, never the value itself, which may be a card
//! verification code or a social security number. Port methods turn it into a
//! [`PortError`] with `?`.

pub mod accounts;
pub mod authorizations;
pub mod cards;
pub mod customers;
pub mod discgrp;
pub mod rewards;
pub mod tcatbal;
pub mod tran_types;
pub mod transactions;
pub mod users;
pub mod xref;

pub use rewards::SoftDeleteOutcome;
pub use transactions::LoadOutcome;

use std::collections::BTreeMap;

use carddemo_domain::numeric::{
    AccountGroupId, AccountId, Alnum, AuthCount, CardNumber, CustSsn, CustomerId, Cvv, FicoScore,
    InterestRate, MerchantId, Money, NumericCode, NumericError, RewardPoints, RewardRate,
    TranCategoryCode, TranId, TranTypeCode, UserId,
};
use carddemo_domain::ports::{PortError, PortResult};
use rust_decimal::Decimal;

use crate::PersistenceError;

/// The misfit of a stored value: the table, the column and the domain
/// constructor's reason. The value is left out on purpose.
fn corrupt(table: &'static str, column: &'static str, err: NumericError) -> PersistenceError {
    PersistenceError::Corrupt {
        table,
        column,
        reason: err.to_string(),
    }
}

/// An `X(N)` item rebuilt from its right-trimmed `VARCHAR(N)` column.
///
/// Trailing spaces in the stored text are dropped before the item is rebuilt,
/// because its padding restores them; every other character is kept, one
/// ISO 8859-1 byte each, as [`Alnum::from_trimmed`] reads back what
/// [`Alnum::trimmed`] wrote. More than `N` characters, or a character above
/// `U+00FF`, is [`PersistenceError::Corrupt`]: the item is never cut the way a
/// COBOL `MOVE` would cut it.
pub(crate) fn alnum<const N: usize>(
    value: &str,
    table: &'static str,
    column: &'static str,
) -> Result<Alnum<N>, PersistenceError> {
    Alnum::from_trimmed(value.trim_end_matches(' ')).map_err(|err| corrupt(table, column, err))
}

/// [`alnum`] for a nullable column, such as the `auth_fraud` columns the Db2
/// DDL leaves nullable. `NULL` stays `None`.
pub(crate) fn opt_alnum<const N: usize>(
    value: Option<&str>,
    table: &'static str,
    column: &'static str,
) -> Result<Option<Alnum<N>>, PersistenceError> {
    value.map(|text| alnum(text, table, column)).transpose()
}

/// An `S9(D)V99` money field from its `NUMERIC(D+2, 2)` column, through
/// [`Money::from_decimal`]: more than `D` integer digits or a non-zero third
/// fraction digit is [`PersistenceError::Corrupt`].
pub(crate) fn money<const D: u8>(
    value: Decimal,
    table: &'static str,
    column: &'static str,
) -> Result<Money<D>, PersistenceError> {
    Money::from_decimal(value).map_err(|err| corrupt(table, column, err))
}

/// [`money`] for a nullable column: the reward rule bounds and the `auth_fraud`
/// amounts. `NULL` stays `None`.
pub(crate) fn opt_money<const D: u8>(
    value: Option<Decimal>,
    table: &'static str,
    column: &'static str,
) -> Result<Option<Money<D>>, PersistenceError> {
    value.map(|amount| money(amount, table, column)).transpose()
}

/// The `S9(04)V99` `DIS-INT-RATE` from its `NUMERIC(6,2)` column, through
/// [`InterestRate::from_decimal`].
pub(crate) fn interest_rate(
    value: Decimal,
    table: &'static str,
    column: &'static str,
) -> Result<InterestRate, PersistenceError> {
    InterestRate::from_decimal(value).map_err(|err| corrupt(table, column, err))
}

/// A reward rule's rate from its `NUMERIC(9,4)` column, through
/// [`RewardRate::from_decimal`]: a negative rate, a fifth fraction digit or a
/// value of `10^5` or more is [`PersistenceError::Corrupt`].
pub(crate) fn reward_rate(
    value: Decimal,
    table: &'static str,
    column: &'static str,
) -> Result<RewardRate, PersistenceError> {
    RewardRate::from_decimal(value).map_err(|err| corrupt(table, column, err))
}

/// Reward points from their `BIGINT` column; a negative count is
/// [`PersistenceError::Corrupt`].
pub(crate) fn reward_points(
    value: i64,
    table: &'static str,
    column: &'static str,
) -> Result<RewardPoints, PersistenceError> {
    RewardPoints::try_from(value).map_err(|err| corrupt(table, column, err))
}

/// A `9(n)` identifier or code from its `SMALLINT`, `INTEGER` or `BIGINT`
/// column. The value widens losslessly to `i64` and enters through the type's
/// `TryFrom<i64>`, so a negative value or one of `10^n` or more is
/// [`PersistenceError::Corrupt`].
fn digits<T>(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<T, PersistenceError>
where
    T: TryFrom<i64, Error = NumericError>,
{
    T::try_from(value.into()).map_err(|err| corrupt(table, column, err))
}

/// An `ACCT-ID 9(11)` from its `BIGINT` column.
pub(crate) fn account_id(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<AccountId, PersistenceError> {
    digits(value, table, column)
}

/// A `CUST-ID 9(09)` from its `INTEGER` column, or from the `BIGINT`
/// `auth_fraud.cust_id`.
pub(crate) fn customer_id(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<CustomerId, PersistenceError> {
    digits(value, table, column)
}

/// A `TRAN-CAT-CD 9(04)` from its `INTEGER` column.
pub(crate) fn tran_category_code(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<TranCategoryCode, PersistenceError> {
    digits(value, table, column)
}

/// A `CARD-CVV-CD 9(03)` from its `INTEGER` column.
pub(crate) fn cvv(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<Cvv, PersistenceError> {
    digits(value, table, column)
}

/// A `TRAN-MERCHANT-ID 9(09)` from its `INTEGER` column.
pub(crate) fn merchant_id(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<MerchantId, PersistenceError> {
    digits(value, table, column)
}

/// A `CUST-SSN 9(09)` from its `INTEGER` column.
pub(crate) fn cust_ssn(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<CustSsn, PersistenceError> {
    digits(value, table, column)
}

/// A `CUST-FICO-CREDIT-SCORE 9(03)` from its `INTEGER` column.
pub(crate) fn fico_score(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<FicoScore, PersistenceError> {
    digits(value, table, column)
}

/// An unsigned `9(N)` code without a type of its own, such as the
/// `PA-PROCESSING-CODE 9(06)` or `PA-POS-ENTRY-MODE 9(02)`, from its
/// `SMALLINT` or `INTEGER` column.
pub(crate) fn numeric_code<const N: u8>(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<NumericCode<N>, PersistenceError> {
    digits(value, table, column)
}

/// An `S9(04) COMP` authorization count from its `SMALLINT` column, or any
/// wider integer column: a magnitude above 9999 is
/// [`PersistenceError::Corrupt`].
pub(crate) fn auth_count(
    value: impl Into<i64>,
    table: &'static str,
    column: &'static str,
) -> Result<AuthCount, PersistenceError> {
    i16::try_from(value.into())
        .map_err(|_| NumericError::OutOfRange)
        .and_then(AuthCount::new)
        .map_err(|err| corrupt(table, column, err))
}

/// A `CARD-NUM X(16)` key from its `VARCHAR(16)` column, as [`alnum`] reads it.
pub(crate) fn card_number(
    value: &str,
    table: &'static str,
    column: &'static str,
) -> Result<CardNumber, PersistenceError> {
    alnum(value, table, column).map(CardNumber::from_alnum)
}

/// A `TRAN-ID X(16)` key from its `VARCHAR(16)` column, as [`alnum`] reads it.
pub(crate) fn tran_id(
    value: &str,
    table: &'static str,
    column: &'static str,
) -> Result<TranId, PersistenceError> {
    alnum(value, table, column).map(TranId::from_alnum)
}

/// A `TRAN-TYPE-CD X(02)` code from its `VARCHAR(2)` column, as [`alnum`]
/// reads it.
pub(crate) fn tran_type_code(
    value: &str,
    table: &'static str,
    column: &'static str,
) -> Result<TranTypeCode, PersistenceError> {
    alnum(value, table, column).map(TranTypeCode::from_alnum)
}

/// A `SEC-USR-ID X(08)` key from its `VARCHAR(8)` column, as [`alnum`] reads
/// it. The ID keeps the case it was stored with.
pub(crate) fn user_id(
    value: &str,
    table: &'static str,
    column: &'static str,
) -> Result<UserId, PersistenceError> {
    alnum(value, table, column).map(UserId::from_alnum)
}

/// An `ACCT-GROUP-ID X(10)` from its `VARCHAR(10)` column, as [`alnum`] reads
/// it. The blank group ID of every fixture account is stored as `''` and comes
/// back as ten spaces (E-06).
pub(crate) fn account_group_id(
    value: &str,
    table: &'static str,
    column: &'static str,
) -> Result<AccountGroupId, PersistenceError> {
    alnum(value, table, column).map(AccountGroupId::from_alnum)
}

/// A row limit as the `BIGINT` a `LIMIT` clause binds.
///
/// Generic over the caller's integer type, so the `u16` of `Browse::limit`
/// and of the batch sweeps converts with no fallible-conversion lint, and a
/// wider type that may exceed `i64::MAX` is reported as
/// [`PortError::Unavailable`] instead of being cut.
pub(crate) fn limit_i64<T>(limit: T) -> PortResult<i64>
where
    i64: TryFrom<T>,
{
    i64::try_from(limit)
        .map_err(|_| PortError::Unavailable("row limit exceeds the BIGINT range".to_owned()))
}

/// The outcome of an `UPDATE` or `DELETE` by key from its affected-row count:
/// no row is [`PortError::NotFound`], the VSAM `NOTFND` or Db2 `SQLCODE` +100
/// the COBOL branches on.
pub(crate) fn expect_affected(rows: u64) -> PortResult<()> {
    if rows == 0 {
        Err(PortError::NotFound)
    } else {
        Ok(())
    }
}

/// The outcome of the keyed reads that one `WHERE <key> = ANY($1)` statement
/// stands for: one entry per requested key, each the outcome the keyed read of
/// that key alone returns.
///
/// `binds` holds each requested key with the value its key column binds, or
/// with the failure of binding it, which is then that key's outcome. `rows` are
/// the rows the statement returned, and `row_key` gives the key column of each.
/// A key no row has is [`PortError::NotFound`], the `NOTFND` (status 23) of
/// its read. A row becomes its record through `record` on its own, so a stored
/// value that does not fit its PIC clause fails that key and no other.
pub(crate) fn keyed_records<K, B, R, T>(
    binds: BTreeMap<K, PortResult<B>>,
    rows: Vec<R>,
    row_key: fn(&R) -> B,
    record: fn(R) -> Result<T, PersistenceError>,
) -> BTreeMap<K, PortResult<T>>
where
    K: Ord,
    B: Ord,
{
    let mut by_key: BTreeMap<B, R> = rows.into_iter().map(|row| (row_key(&row), row)).collect();
    binds
        .into_iter()
        .map(|(key, bind)| {
            let outcome = bind.and_then(|value| {
                let row = by_key.remove(&value).ok_or(PortError::NotFound)?;
                record(row).map_err(PortError::from)
            });
            (key, outcome)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fmt::Display;

    use carddemo_domain::numeric::{AuthCount, FicoScore};
    use carddemo_domain::ports::PortError;
    use rust_decimal::Decimal;

    use super::{
        PersistenceError, account_group_id, account_id, alnum, auth_count, card_number, cust_ssn,
        customer_id, cvv, expect_affected, fico_score, interest_rate, keyed_records, limit_i64,
        merchant_id, money, numeric_code, opt_alnum, opt_money, reward_points, reward_rate,
        tran_category_code, tran_id, tran_type_code, user_id,
    };

    const TABLE: &str = "accounts";
    const COLUMN: &str = "acct_group_id";

    // The `NumericError` texts the domain constructors report.
    const TOO_LONG: &str = "expected at most 3 characters, got 4";
    const NOT_LATIN1: &str = "value has a character outside ISO 8859-1";
    const OUT_OF_RANGE: &str = "value is outside the range of the field";
    const TOO_MANY_DECIMALS: &str = "value has more decimal places than the field allows";
    const NEGATIVE: &str = "value must not be negative";

    /// A conversion reduced to the text of its value, or to the reason of its
    /// `Corrupt` error once that error is checked to name `TABLE` and `COLUMN`.
    fn outcome<T: Display>(result: Result<T, PersistenceError>) -> Result<String, String> {
        match result {
            Ok(value) => Ok(value.to_string()),
            Err(PersistenceError::Corrupt {
                table,
                column,
                reason,
            }) => {
                assert_eq!((table, column), (TABLE, COLUMN));
                Err(reason)
            }
            Err(other) => panic!("expected a Corrupt error, got {other}"),
        }
    }

    /// [`outcome`] for a nullable column, with `NULL` for `None`.
    fn opt_outcome<T: Display>(
        result: Result<Option<T>, PersistenceError>,
    ) -> Result<String, String> {
        outcome(result.map(|value| value.map_or_else(|| "NULL".to_owned(), |v| v.to_string())))
    }

    /// A named conversion: its [`outcome`] and the value text or reason expected.
    type Case<'a> = (&'a str, Result<String, String>, Result<&'a str, &'a str>);

    fn owned(expected: Result<&str, &str>) -> Result<String, String> {
        expected.map(str::to_owned).map_err(str::to_owned)
    }

    fn assert_cases(cases: &[Case<'_>]) {
        for (name, actual, expected) in cases {
            assert_eq!(actual, &owned(*expected), "{name}");
        }
    }

    #[test]
    fn text_columns_are_repadded_and_never_cut() {
        let cases: [(&str, Result<&str, &str>); 8] = [
            ("AB", Ok("AB ")),
            ("", Ok("   ")),
            ("ABC", Ok("ABC")),
            ("AB ", Ok("AB ")),
            ("ABC  ", Ok("ABC")),
            ("\u{e9}", Ok("\u{e9}  ")),
            ("ABCD", Err(TOO_LONG)),
            ("A\u{100}", Err(NOT_LATIN1)),
        ];
        for (stored, expected) in cases {
            let actual = outcome(alnum::<3>(stored, TABLE, COLUMN));
            assert_eq!(actual, owned(expected), "{stored:?}");
        }
        let latin1 = alnum::<3>("\u{e9}", TABLE, COLUMN).map(|item| *item.as_bytes());
        assert!(matches!(latin1, Ok([0xE9, b' ', b' '])), "{latin1:?}");
    }

    #[test]
    fn text_keys_are_repadded_and_never_cut() {
        assert_cases(&[
            (
                "card_number",
                outcome(card_number("4111111111111111", TABLE, COLUMN)),
                Ok("4111111111111111"),
            ),
            (
                "tran_id",
                outcome(tran_id("0000000000000001", TABLE, COLUMN)),
                Ok("0000000000000001"),
            ),
            (
                "tran_type_code",
                outcome(tran_type_code("1", TABLE, COLUMN)),
                Ok("1 "),
            ),
            (
                "tran_type_code too long",
                outcome(tran_type_code("011", TABLE, COLUMN)),
                Err("expected at most 2 characters, got 3"),
            ),
            (
                "user_id keeps case",
                outcome(user_id("user01", TABLE, COLUMN)),
                Ok("user01  "),
            ),
            (
                "blank group id (E-06)",
                outcome(account_group_id("", TABLE, COLUMN)),
                Ok("          "),
            ),
            (
                "default group id",
                outcome(account_group_id("DEFAULT", TABLE, COLUMN)),
                Ok("DEFAULT   "),
            ),
        ]);
    }

    #[test]
    fn money_and_rates_must_fit_their_picture() {
        assert_cases(&[
            (
                "S9(09)V99 maximum",
                outcome(money::<9>(Decimal::new(99_999_999_999, 2), TABLE, COLUMN)),
                Ok("999999999.99"),
            ),
            (
                "S9(09)V99 minimum",
                outcome(money::<9>(Decimal::new(-99_999_999_999, 2), TABLE, COLUMN)),
                Ok("-999999999.99"),
            ),
            (
                "S9(09)V99 trailing zero",
                outcome(money::<9>(Decimal::new(12_340, 3), TABLE, COLUMN)),
                Ok("12.34"),
            ),
            (
                "S9(09)V99 ten integer digits",
                outcome(money::<9>(Decimal::new(100_000_000_000, 2), TABLE, COLUMN)),
                Err(OUT_OF_RANGE),
            ),
            (
                "S9(09)V99 third fraction digit",
                outcome(money::<9>(Decimal::new(1_001, 3), TABLE, COLUMN)),
                Err(TOO_MANY_DECIMALS),
            ),
            (
                "S9(10)V99 ten integer digits",
                outcome(money::<10>(Decimal::new(999_999_999_999, 2), TABLE, COLUMN)),
                Ok("9999999999.99"),
            ),
            (
                "S9(04)V99 rate",
                outcome(interest_rate(Decimal::new(1_500, 2), TABLE, COLUMN)),
                Ok("15.00"),
            ),
            (
                "S9(04)V99 rate five integer digits",
                outcome(interest_rate(Decimal::new(10_000, 0), TABLE, COLUMN)),
                Err(OUT_OF_RANGE),
            ),
            (
                "reward rate",
                outcome(reward_rate(Decimal::new(15, 1), TABLE, COLUMN)),
                Ok("1.5000"),
            ),
            (
                "reward rate negative",
                outcome(reward_rate(Decimal::new(-1, 0), TABLE, COLUMN)),
                Err(NEGATIVE),
            ),
            (
                "reward rate fifth fraction digit",
                outcome(reward_rate(Decimal::new(1, 5), TABLE, COLUMN)),
                Err(TOO_MANY_DECIMALS),
            ),
        ]);
    }

    #[test]
    fn nullable_columns_keep_null_as_none() {
        assert_cases(&[
            (
                "text NULL",
                opt_outcome(opt_alnum::<3>(None, TABLE, COLUMN)),
                Ok("NULL"),
            ),
            (
                "text",
                opt_outcome(opt_alnum::<3>(Some("AB"), TABLE, COLUMN)),
                Ok("AB "),
            ),
            (
                "text too long",
                opt_outcome(opt_alnum::<3>(Some("ABCD"), TABLE, COLUMN)),
                Err(TOO_LONG),
            ),
            (
                "money NULL",
                opt_outcome(opt_money::<9>(None, TABLE, COLUMN)),
                Ok("NULL"),
            ),
            (
                "money",
                opt_outcome(opt_money::<9>(Some(Decimal::new(-5, 2)), TABLE, COLUMN)),
                Ok("-0.05"),
            ),
            (
                "money third fraction digit",
                opt_outcome(opt_money::<9>(Some(Decimal::new(5, 3)), TABLE, COLUMN)),
                Err(TOO_MANY_DECIMALS),
            ),
        ]);
    }

    #[test]
    fn integer_columns_widen_losslessly_and_must_fit_their_digits() {
        assert_cases(&[
            (
                "account_id zero",
                outcome(account_id(0_i64, TABLE, COLUMN)),
                Ok("00000000000"),
            ),
            (
                "account_id eleven digits",
                outcome(account_id(99_999_999_999_i64, TABLE, COLUMN)),
                Ok("99999999999"),
            ),
            (
                "account_id negative",
                outcome(account_id(-1_i64, TABLE, COLUMN)),
                Err(NEGATIVE),
            ),
            (
                "account_id 10^11",
                outcome(account_id(100_000_000_000_i64, TABLE, COLUMN)),
                Err(OUT_OF_RANGE),
            ),
            (
                "customer_id INTEGER",
                outcome(customer_id(1_i32, TABLE, COLUMN)),
                Ok("000000001"),
            ),
            (
                "customer_id BIGINT 10^9",
                outcome(customer_id(1_000_000_000_i64, TABLE, COLUMN)),
                Err(OUT_OF_RANGE),
            ),
            (
                "category",
                outcome(tran_category_code(5_i32, TABLE, COLUMN)),
                Ok("0005"),
            ),
            (
                "category 10^4",
                outcome(tran_category_code(10_000_i32, TABLE, COLUMN)),
                Err(OUT_OF_RANGE),
            ),
            (
                "cvv 10^3",
                outcome(cvv(1_000_i32, TABLE, COLUMN)),
                Err(OUT_OF_RANGE),
            ),
            (
                "merchant",
                outcome(merchant_id(800_000_000_i32, TABLE, COLUMN)),
                Ok("800000000"),
            ),
            (
                "ssn negative",
                outcome(cust_ssn(-5_i32, TABLE, COLUMN)),
                Err(NEGATIVE),
            ),
            (
                "fico",
                outcome(fico_score(750_i32, TABLE, COLUMN)),
                Ok("750"),
            ),
            (
                "pos entry mode SMALLINT",
                outcome(numeric_code::<2>(5_i16, TABLE, COLUMN)),
                Ok("05"),
            ),
            (
                "pos entry mode 10^2",
                outcome(numeric_code::<2>(100_i32, TABLE, COLUMN)),
                Err(OUT_OF_RANGE),
            ),
        ]);
    }

    #[test]
    fn counts_and_points_must_fit_their_fields() {
        let count = |value: i32| outcome(auth_count(value, TABLE, COLUMN).map(AuthCount::get));
        let points = |value: i64| outcome(reward_points(value, TABLE, COLUMN));
        assert_cases(&[
            ("count 9999", count(9_999), Ok("9999")),
            ("count -9999", count(-9_999), Ok("-9999")),
            ("count 10000", count(10_000), Err(OUT_OF_RANGE)),
            ("count beyond SMALLINT", count(40_000), Err(OUT_OF_RANGE)),
            ("points zero", points(0), Ok("0")),
            (
                "points above 2^53",
                points(9_007_199_254_740_993),
                Ok("9007199254740993"),
            ),
            ("points negative", points(-1), Err(NEGATIVE)),
        ]);
    }

    #[test]
    fn a_misfit_reaches_the_port_as_unavailable_without_the_value() {
        let error = match cust_ssn(1_234_567_890_i64, "customers", "cust_ssn") {
            Err(error) => PortError::from(error),
            Ok(ssn) => panic!("a ten-digit SSN was accepted: {ssn:?}"),
        };
        let expected =
            format!("stored customers.cust_ssn value does not fit its domain type: {OUT_OF_RANGE}");
        assert_eq!(error, PortError::Unavailable(expected));
        assert!(!error.to_string().contains("1234567890"), "{error}");
    }

    #[test]
    fn limits_convert_to_bigint_without_cutting() {
        assert_eq!(limit_i64(u16::MAX), Ok(65_535));
        assert_eq!(limit_i64(0_u16), Ok(0));
        assert_eq!(
            limit_i64(u64::MAX),
            Err(PortError::Unavailable(
                "row limit exceeds the BIGINT range".to_owned()
            ))
        );
    }

    #[test]
    fn an_update_or_delete_of_no_row_is_not_found() {
        for (rows, expected) in [(0, Err(PortError::NotFound)), (1, Ok(())), (2, Ok(()))] {
            assert_eq!(expect_affected(rows), expected, "{rows} rows");
        }
    }

    #[test]
    fn a_batch_of_keyed_reads_gives_each_key_the_outcome_of_its_own_read() {
        let unbound = PortError::Unavailable("value exceeds the INTEGER range".to_owned());
        let binds = BTreeMap::from([
            ("fits", Ok(1)),
            ("no row", Ok(2)),
            ("misfit", Ok(3)),
            ("unbound", Err(unbound.clone())),
            ("last", Ok(5)),
        ]);
        // Rows in no particular order, as `= ANY($1)` returns them.
        let rows = vec![(5, 999), (3, 1_000), (1, 274)];
        let outcomes = keyed_records(
            binds,
            rows,
            |row| row.0,
            |row| fico_score(row.1, "customers", "cust_fico_credit_score"),
        );
        let misfit = format!(
            "stored customers.cust_fico_credit_score value does not fit its domain type: \
             {OUT_OF_RANGE}"
        );
        assert_eq!(
            outcomes,
            BTreeMap::from([
                ("fits", Ok(FicoScore::new(274).expect("274 fits 9(03)"))),
                ("no row", Err(PortError::NotFound)),
                ("misfit", Err(PortError::Unavailable(misfit))),
                ("unbound", Err(unbound)),
                ("last", Ok(FicoScore::new(999).expect("999 fits 9(03)"))),
            ])
        );
    }
}
