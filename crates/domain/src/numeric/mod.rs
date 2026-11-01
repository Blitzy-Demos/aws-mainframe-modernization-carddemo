//! Newtype algebra for the copybook numeric and text fields (request R18).
//!
//! Every numeric field of a copybook record maps to a semantic type fixed by its
//! sign, integer width and scale, and every alphanumeric field to a
//! fixed-capacity text type. Fields that share a PIC clause and a semantic class
//! share a type, because the COBOL adds them to one another (for example
//! `ADD DALYTRAN-AMT TO ACCT-CURR-BAL`); fields of different classes have
//! different types, so mixing them fails to compile. Every consumer imports these
//! types from this module and defines no copy of its own.
//!
//! Widths and scales come from the reviewed rows of section A (record layouts) and
//! section C (arithmetic) of `docs/parity/parity-oracle-audit.md`; the row IDs
//! below are that audit's.
//!
//! # PIC clause to type
//!
//! | PIC class | Type | SQL column | Audit rows (examples) |
//! |---|---|---|---|
//! | `S9(n)V99` money, DISPLAY or COMP-3 | [`Money<n>`](Money): signed, scale 2, `n` integer digits | `NUMERIC(n+2, 2)` | `ACCT-CURR-BAL S9(10)V99` P-CVACT01Y-003; `TRAN-AMT S9(09)V99` P-CVTRA05Y-006; `PA-CREDIT-LIMIT S9(09)V99 COMP-3` P-CIPAUSMY-005 |
//! | `S9(04)V99` annual interest rate | [`InterestRate`] | `NUMERIC(6,2)` | `DIS-INT-RATE` P-CVTRA02Y-004 |
//! | reward rate (new) | [`RewardRate`]: non-negative, scale at most 4 | `NUMERIC(9,4)` | none (no COBOL oracle) |
//! | reward points (new) | [`RewardPoints`]: whole, non-negative, within the `i64` range | `BIGINT` | none (no COBOL oracle) |
//! | `9(n)` identifiers and codes | [`AccountId`], [`CustomerId`], [`TranCategoryCode`], [`Cvv`], [`MerchantId`], [`CustSsn`], [`FicoScore`], [`NumericCode`]; no arithmetic | `BIGINT` or `INTEGER` with `CHECK (0 <= v < 10^n)` | `ACCT-ID 9(11)` P-CVACT01Y-001; `TRAN-CAT-CD 9(04)` P-CVTRA05Y-003 |
//! | `S9(04) COMP` counters | [`AuthCount`]: checked increment and decrement only | `SMALLINT` | `PA-APPROVED-AUTH-CNT` P-CIPAUSMY-009; `PA-DECLINED-AUTH-CNT` P-CIPAUSMY-010 |
//! | `X(n)` keys and codes | [`CardNumber`], [`TranId`], [`TranTypeCode`], [`UserId`], [`AccountGroupId`] | `VARCHAR(n)` | `CARD-NUM X(16)` P-CVACT02Y-001; `TRAN-ID X(16)` P-CVTRA05Y-001 |
//! | other `X(n)`: names, addresses, dates, timestamps | [`Alnum<n>`](Alnum): the stored bytes, trailing spaces kept | `VARCHAR(n)`, right-trimmed | `ACCT-OPEN-DATE X(10)` P-CVACT01Y-006; `TRAN-ORIG-TS X(26)` P-CVTRA05Y-012 |
//! | row identities (new) | [`TransactionRowId`], [`RuleId`], [`AccrualId`], [`AuthId`] | `BIGINT` identity | none (not copybook fields) |
//!
//! Some section A rows label their Rust type with a name this module spells
//! differently. The widths agree, and the types below are the ones to use:
//!
//! - `Ssn` (P-CVCUS01Y-013) is [`CustSsn`].
//! - `ProcessingCode`, `PosEntryMode`, `RejectReason`, `MqConditionCode` and
//!   `MqReasonCode` (P-CIPAUDTY-013, P-CIPAUDTY-018, P-CBTRN02C-002,
//!   P-COACCT01-005, P-COACCT01-007 and their counterparts) are the
//!   [`NumericCode`] of their PIC width: 6, 2, 4, 2 and 5 digits.
//! - `i16 (count)` (P-CIPAUSMY-009, P-CIPAUSMY-010) is [`AuthCount`].
//!
//! # Permitted arithmetic
//!
//! | Operands | Result | COBOL meaning |
//! |---|---|---|
//! | `Money<A> ± Money<B>`, `Money<A> ± MoneyExpr`, `MoneyExpr ± Money<B>`, `MoneyExpr ± MoneyExpr` | [`MoneyExpr`], exact | `ADD`, `SUBTRACT` or `COMPUTE` intermediate |
//! | `MoneyExpr::store::<D>()`, `Money<A>::store::<D>()` | `Money<D>`: excess fraction digits and high-order integer digits truncated, sign kept | receiving field without `ON SIZE ERROR`, numeric `MOVE` (P-COBIL00C-021) |
//! | `MoneyExpr::store_checked::<D>()` | `Result<Money<D>, Overflow>` | receiving field with `ON SIZE ERROR` |
//! | `Money<D> * InterestRate` | [`RatedBalance`], the exact product; its only method, `monthly()`, gives the cent quotient of the division by 1200, truncated toward zero, as a `MoneyExpr` | `CBACT04C` `1300-COMPUTE-INTEREST` (P-CBACT04C-012) |
//! | `Money<D> * RewardRate` | [`PointsExpr`], exact; its only method, `ceil()`, gives `Result<RewardPoints, Overflow>` | reward accrual (new) |
//! | `RewardPoints ± RewardPoints` | `Result<RewardPoints, Overflow>` | reward balance totals (new) |
//! | comparisons | `Money` and `MoneyExpr` among themselves, across widths | `IF ... >= ...` |
//!
//! Anything not listed has no `impl` and fails to compile. The compile-fail suite
//! `crates/domain/tests/newtype_algebra.rs` proves it for money plus an interest
//! rate, money plus a reward rate, money plus reward points, reward points plus
//! money, and money built from a floating-point literal.
//!
//! No type here rounds. No COBOL program or copybook in the repository uses
//! `ROUNDED` (P-CBACT04C-011), so every store truncates, and the only rounding in
//! the algebra is the `ceil` the reward rule prescribes for whole points. No type
//! converts from any floating-point type: values enter as `Decimal`, as integer
//! units or as digit text, and are validated where they enter.

pub mod alnum;
pub mod ids;
pub mod money;
pub mod points;
pub mod rates;

pub use alnum::Alnum;
pub use ids::{
    AccountGroupId, AccountId, AccrualId, AuthCount, AuthId, CardNumber, CustSsn, CustomerId, Cvv,
    FicoScore, MerchantId, NumericCode, RuleId, TranCategoryCode, TranId, TranTypeCode,
    TransactionRowId, UserId,
};
pub use money::{Money, MoneyExpr};
pub use points::{PointsExpr, RewardPoints};
pub use rates::{InterestRate, RatedBalance, RewardRate};

/// The COBOL size-error condition: a result does not fit its receiving field.
///
/// Returned where the algebra reports an overflow instead of truncating:
///
/// - `MoneyExpr::store_checked`, the `ON SIZE ERROR` form of a store, when the
///   integer part does not fit the receiver's digits;
/// - `PointsExpr::ceil` and the checked `RewardPoints` additions and
///   subtractions, when the points fall outside `0..=i64::MAX`;
/// - the checked `AuthCount` increment and decrement, when a count passes the
///   four digits of `S9(04) COMP`. The COBOL `ADD 1` and `SUBTRACT 1` there have
///   no `ON SIZE ERROR`, and their wrap depends on a compiler option the
///   repository does not fix, so the overflow is reported rather than guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("value does not fit the receiving field")]
pub struct Overflow;

/// Why a value cannot be held by a numeric or text field type.
///
/// Constructors return it where a value enters the domain: a decoded record
/// field, an SQL column or request text. Arithmetic inside the algebra reports
/// [`Overflow`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NumericError {
    /// More integer digits than the field's PIC clause allows. Returned by
    /// `Money::from_decimal` and `Money::from_cents`,
    /// `InterestRate::from_decimal` and `InterestRate::from_hundredths`,
    /// `RewardRate::from_decimal`, the `new`, `TryFrom<i64>` and `FromStr` of the
    /// `9(n)` identifiers and of `NumericCode`, `AuthCount::new`, and
    /// `RewardPoints::from_str` above `i64::MAX`.
    #[error("value is outside the range of the field")]
    OutOfRange,
    /// More fraction digits than the field's scale. Returned by
    /// `Money::from_decimal` and `InterestRate::from_decimal` (scale 2) and by
    /// `RewardRate::from_decimal` (scale 4); excess digits are never dropped.
    #[error("value has more decimal places than the field allows")]
    TooManyDecimals,
    /// A negative value for an unsigned field. Returned by
    /// `RewardRate::from_decimal`, `RewardPoints::try_from` and the
    /// `TryFrom<i64>` of the `9(n)` identifiers.
    #[error("value must not be negative")]
    Negative,
    /// Text that is not a digit string. Returned by the `FromStr` of the `9(n)`
    /// identifiers, of `NumericCode` and of `RewardPoints`, and by `TranId::next`
    /// on a key that is not 16 digits.
    #[error("value is not numeric")]
    NotNumeric,
    /// A byte slice whose length differs from the field's. Returned by
    /// `Alnum::from_bytes`.
    #[error("expected {expected} bytes, got {actual}")]
    WrongLength {
        /// The field length in bytes: the `N` of `X(N)`.
        expected: usize,
        /// The length of the slice offered.
        actual: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::{NumericError, Overflow};

    #[test]
    fn overflow_reports_the_size_error_text() {
        assert_eq!(
            Overflow.to_string(),
            "value does not fit the receiving field"
        );
    }

    #[test]
    fn numeric_error_variants_report_their_texts() {
        let cases = [
            (
                NumericError::OutOfRange,
                "value is outside the range of the field",
            ),
            (
                NumericError::TooManyDecimals,
                "value has more decimal places than the field allows",
            ),
            (NumericError::Negative, "value must not be negative"),
            (NumericError::NotNumeric, "value is not numeric"),
            (
                NumericError::WrongLength {
                    expected: 16,
                    actual: 3,
                },
                "expected 16 bytes, got 3",
            ),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text, "{error:?}");
        }
    }

    #[test]
    fn errors_are_standard_errors_without_a_source() {
        let errors: [&(dyn std::error::Error + Send + Sync); 2] =
            [&Overflow, &NumericError::NotNumeric];
        for error in errors {
            assert!(error.source().is_none(), "{error}");
        }
    }
}
